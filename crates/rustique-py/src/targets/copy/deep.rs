//! `copy.deepcopy`, with the whole recursion inside the kernel, for the call trees in which
//! no user code runs.
//!
//! The engine executes `Lib/copy.py`'s algorithm step for step — the same memo writes and
//! keep-alive appends, the iteration protocol, the evaluation order (`y[deepcopy(k)] =
//! deepcopy(v)` copies the value first), the same allocation pattern of the built
//! containers — with an explicit work stack instead of C recursion. Every step it takes is
//! *pure*: it runs no Python code and calls no hook. At the first node that would run user
//! code (a `__deepcopy__` that is not a recognised no-op, a registered copier or reductor,
//! a custom reduce protocol, `__setstate__`, `__new__`, `__del__` on a copy, a key whose
//! `__hash__`/`__eq__` is not a builtin, a metaclass hook, ...), at a recursion depth where
//! CPython would raise, or on any error, it abandons the partial copies (whose release runs
//! no user code either) and the call restarts in CPython's original with the unchanged
//! arguments (contract §5.1). So hooks always run under CPython's own frames.
//!
//! Native: atomic types, `list`/`tuple`/`dict`/`frozendict`/bound methods through the
//! original helpers, `issubclass(cls, type)`, `__deepcopy__` methods that are exactly
//! `return self` (Enum), plain objects and dataclasses (`object.__reduce_ex__(4)` emulated,
//! `reduce.rs`), exact `set`/`frozenset`, and the recognised C types of `known.rs`
//! (`datetime` family, `Decimal`).

use std::ptr::null_mut;

use pyo3::ffi;

use super::known::{Known, datetime_reduce, known};
use super::memo::Memo;
use super::reduce::{object_new_alloc, plain_reduce};
use super::rt::*;

enum K {
    /// `_deepcopy_list`: `y` (strong) built by appends; next index `i`.
    List { y: O, i: ffi::Py_ssize_t },
    /// `_deepcopy_tuple`: the list comprehension of copies (null while all are atomic).
    Tuple { y: O, i: ffi::Py_ssize_t },
    /// `_deepcopy_dict` / `_deepcopy_frozendict`: items iterator; `key` waits (strong) while
    /// its value is copied, then `vcopy` (strong) waits while the key is copied.
    Dict { y: O, it: DictIt, key: O, vcopy: O, frozen: bool },
    /// `_deepcopy_method`: waiting for the copy of `__self__`.
    Method,
    /// `_reconstruct(x, memo, *rv)` for a natively computed reduce value.
    Recon(Option<Box<Recon>>),
}

struct Frame {
    x: O,    // strong
    hold: O, // strong or null: a child kept alive, or the reduce value
    helper: bool,
    k: K,
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: owned references.
        unsafe {
            match &mut self.k {
                K::List { y, .. } | K::Tuple { y, .. } => xdecref(*y),
                K::Dict { y, key, vcopy, .. } => {
                    xdecref(*vcopy);
                    xdecref(*key);
                    xdecref(*y);
                }
                K::Method | K::Recon(_) => {}
            }
            xdecref(self.hold);
            decref(self.x);
        }
    }
}

/// `_reconstruct` state for `(func, args, state)` with `func` `copyreg.__newobj__` (plain
/// objects, `args == (cls,)`), `set`/`frozenset` (`(list,)`), or a recognised `_datetime`
/// type (its C constructor).
struct Recon {
    func: O,
    args: O,  // borrowed tuple (the reduce value is held by Frame::hold)
    state: O, // borrowed from the reduce value
    phase: u8,
    i: ffi::Py_ssize_t,
    acopy: O, // strong: the list of argument copies
    y: O,     // strong
}

impl Drop for Recon {
    fn drop(&mut self) {
        // SAFETY: owned references.
        unsafe {
            xdecref(self.y);
            xdecref(self.acopy);
        }
    }
}

const P_ARG: u8 = 1;
const P_STATE: u8 = 2;

enum Began {
    Ready(O),
    Pushed,
}

enum Step {
    /// Copy this object next (borrowed: kept alive by the frame).
    Child(O),
    Done(O),
}

pub struct Engine {
    st: &'static St,
    /// `py_recursion_remaining` when the emulated calls started, and the emulated Python
    /// frames entered since: CPython raises where `budget - used` would drop to 0.
    budget: i32,
    used: i32,
    memo: Memo,
    stack: SVec<Frame, 8>,
    /// Helpers verified intact (bit i) / modified (bit 16 + i) in this call.
    checked: u32,
    atomic_fast: bool,
    /// The root's frame was entered: a restart from here on abandons work.
    started: bool,
}

impl Engine {
    /// `memo`: null (`deepcopy(x)`) or the caller's exact dict (borrowed). `budget`: the
    /// recursion budget at the call (`py_recursion_remaining`).
    #[inline(always)]
    pub unsafe fn new(st: &'static St, memo: O, atomic_fast: bool, budget: i32) -> Engine {
        Engine {
            st,
            budget,
            used: 0,
            // SAFETY: a live dict or null.
            memo: unsafe { Memo::new(memo) },
            stack: SVec::new(),
            checked: 0,
            atomic_fast,
            started: false,
        }
    }

    /// Copy `root`; `Err(abandoned)`: restart in the original (no exception is left set);
    /// `abandoned` is false when nothing was built yet (user code at the root itself).
    pub unsafe fn run(mut self, root: O) -> Result<O, bool> {
        // SAFETY: attached thread.
        let r = unsafe { self.drive(root) };
        let r = match r {
            // SAFETY: as above.
            Ok(y) => match unsafe { self.memo.commit() } {
                Ok(()) => Ok(y),
                Err(()) => {
                    // SAFETY: owned.
                    unsafe { decref(y) };
                    Err(true)
                }
            },
            Err(()) => Err(self.started),
        };
        // Partial copies go first, then the memo (as CPython's frames and dict would).
        while let Some(f) = self.stack.pop() {
            drop(f);
        }
        drop(self);
        if r.is_err() {
            // SAFETY: attached thread.
            unsafe { ffi::PyErr_Clear() };
        }
        r
    }

    #[inline(always)]
    fn push(&mut self) -> R<()> {
        if self.budget - self.used <= 0 {
            return Err(());
        }
        self.used += 1;
        Ok(())
    }

    #[inline(always)]
    fn pop(&mut self) {
        self.used -= 1;
    }

    /// A Python frame CPython would enter and leave (only its depth check matters).
    #[inline(always)]
    fn touch(&self) -> R<()> {
        if self.budget - self.used <= 0 { Err(()) } else { Ok(()) }
    }

    #[inline(always)]
    fn helper(&mut self, i: usize) -> bool {
        if self.checked & (1 << i) != 0 {
            return true;
        }
        if self.checked & (1 << (16 + i)) != 0 {
            return false;
        }
        // SAFETY: attached thread.
        let ok = unsafe { helper_intact(self.st, i) };
        self.checked |= if ok { 1 << i } else { 1 << (16 + i) };
        ok
    }

    /// `cls in _atomic_types`, decided purely (Err: the lookup could run user code).
    #[inline(always)]
    unsafe fn atomic(&self, cls: *mut ffi::PyTypeObject) -> R<bool> {
        if is_quick(cls) && self.atomic_fast {
            return Ok(true);
        }
        // SAFETY: live type object.
        if !self.atomic_fast || unsafe { meta_hash(cls.cast()) } != default_type_hash() {
            return Err(());
        }
        Ok(self.st.atomic_types[5..].contains(&cls))
    }

    unsafe fn drive(&mut self, root: O) -> R<O> {
        // SAFETY: attached thread; every object handled is kept alive by a frame.
        unsafe {
            let mut input: O = match self.begin(root, true)? {
                Began::Ready(y) => return Ok(y),
                Began::Pushed => null_mut(),
            };
            self.started = true;
            loop {
                match self.step(input)? {
                    Step::Child(c) => match self.begin(c, false)? {
                        Began::Ready(y) => input = y,
                        Began::Pushed => input = null_mut(),
                    },
                    Step::Done(y) => {
                        let f = self.stack.pop().expect("frame");
                        let y = self.finish(f, y)?;
                        if self.stack.is_empty() {
                            return Ok(y);
                        }
                        input = y;
                    }
                }
            }
        }
    }

    /// The end of `deepcopy(x)`: `if y is not x: memo[d] = y; _keep_alive(x, memo)`.
    #[inline(always)]
    unsafe fn epilogue(&mut self, x: O, y: O) -> R<()> {
        if y != x {
            if !self.helper(H_KEEP_ALIVE) {
                return Err(());
            }
            // SAFETY: live objects.
            unsafe {
                self.memo.set(x, y);
                self.touch()?;
                self.memo.keep_alive(x)?;
            }
        }
        Ok(())
    }

    /// A deepcopy call that produced `y` without a work frame (`y` owned).
    #[inline(always)]
    unsafe fn ready(&mut self, x: O, y: O) -> R<Began> {
        let y = Own(y);
        // SAFETY: live objects.
        unsafe { self.epilogue(x, y.get())? };
        self.pop();
        Ok(Began::Ready(y.into_raw()))
    }

    /// The start of `deepcopy(x, memo)`; `x` borrowed. On Err the frames entered are
    /// irrelevant: the whole call restarts.
    unsafe fn begin(&mut self, x: O, root: bool) -> R<Began> {
        let st = self.st;
        // SAFETY: attached thread; x alive for the duration (the caller's frame holds it).
        unsafe {
            self.push()?;
            let cls = ty(x);
            if self.atomic(cls)? {
                self.pop();
                return Ok(Began::Ready(incref(x)));
            }
            // `memo = {}` at the root without a memo; `if d in memo: return memo[d]`.
            if !root || self.memo.has_base() {
                let y = self.memo.lookup(x)?;
                if !y.is_null() {
                    self.pop();
                    return Ok(Began::Ready(y));
                }
            }
            let copier = Own(dict_get(st.dispatch, cls.cast())?);
            let c = copier.get();
            if !c.is_null() {
                let native = if c == st.h_list {
                    self.helper(H_LIST)
                } else if c == st.h_dict {
                    self.helper(H_DICT)
                } else if c == st.h_tuple {
                    self.helper(H_TUPLE)
                } else if c == st.h_frozendict {
                    self.helper(H_FROZENDICT)
                } else if c == st.h_method {
                    self.helper(H_METHOD)
                } else {
                    false
                };
                if !native {
                    return Err(()); // a registered copier: user code
                }
                return self.push_helper(x, c);
            }
            if ffi::PyType_IsSubtype(cls, &raw mut ffi::PyType_Type) != 0 {
                return self.ready(x, incref(x));
            }
            if let Some(y) = self.deepcopy_hook(x, cls)? {
                return self.ready(x, y);
            }
            // `dispatch_table.get(cls)`: any reductor is user code.
            let r = dict_get(st.dispatch_table, cls.cast())?;
            if !r.is_null() {
                decref(r);
                return Err(());
            }
            if (cls == &raw mut ffi::PySet_Type || cls == &raw mut ffi::PyFrozenSet_Type) && self.helper(H_SLOTNAMES) {
                // `object.__reduce_ex__` -> `set.__reduce__`: `(type(x), (list(x),), None)`
                // once `_PyType_GetSlotNames` called `copyreg._slotnames(cls)` (which returns
                // [] and fails to cache it on the static type): only its frame remains.
                let keys = Own::new(ffi::PySequence_List(x))?;
                self.touch()?;
                let args = Own::new(ffi::PyTuple_Pack(1, keys.get()))?;
                let rv = Own::new(ffi::PyTuple_Pack(3, cls.cast::<ffi::PyObject>(), args.get(), ffi::Py_None()))?;
                return self.reconstruct(x, rv.into_raw());
            }
            if let Some(rv) = plain_reduce(st, x, cls, true, self.touch().is_ok())? {
                return self.reconstruct(x, rv);
            }
            // `_datetime` objects: their C `__reduce_ex__`, then `_reconstruct`.
            if let Some(k) = known(cls)
                && let Some(rv) = datetime_reduce(st, x, cls, k, self.touch().is_ok() && self.helper(H_SLOTNAMES))?
            {
                return self.reconstruct(x, rv);
            }
            Err(())
        }
    }

    /// `copier = getattr(x, "__deepcopy__", None)` decided purely: `Ok(None)` when absent
    /// (or None), the copy when it is a `return self` method; Err otherwise (user code).
    #[inline(always)]
    unsafe fn deepcopy_hook(&self, x: O, cls: *mut ffi::PyTypeObject) -> R<Option<O>> {
        let st = self.st;
        // SAFETY: live objects; generic attribute access only.
        unsafe {
            if !generic_getattr(cls) {
                return Err(());
            }
            let d = _PyType_Lookup(cls, st.s_deepcopy);
            if !d.is_null() && ty(d) == &raw mut ffi::PyMethodDescr_Type && known(cls) == Some(Known::Decimal) {
                // `_decimal.Decimal.__deepcopy__`: C, returns `self`.
                return Ok(Some(incref(x)));
            }
            if !d.is_null() && d != ffi::Py_None() && ty(d) != &raw mut ffi::PyFunction_Type {
                return Err(()); // a descriptor whose `__get__` could be user code
            }
            let m = Own(getattr_opt(x, st.s_deepcopy)?);
            let m = m.get();
            if m.is_null() || m == ffi::Py_None() {
                return Ok(None);
            }
            if ty(m) == &raw mut PyMethod_Type && is_return_self(method_func(m), 2) {
                // `def __deepcopy__(self, memo): return self` (Enum): its frame only.
                self.touch()?;
                return Ok(Some(incref(method_self(m))));
            }
            Err(())
        }
    }

    /// `y = _reconstruct(x, memo, *rv)` for a native reduce value (owned).
    unsafe fn reconstruct(&mut self, x: O, rv: O) -> R<Began> {
        let rv = Own(rv);
        if !self.helper(H_RECONSTRUCT) {
            return Err(());
        }
        // SAFETY: a validated 2-, 3- or 5-tuple `(func, args_tuple, [state, ...])`.
        unsafe {
            let r = rv.get();
            let rec = Box::new(Recon {
                func: ffi::PyTuple_GET_ITEM(r, 0),
                args: ffi::PyTuple_GET_ITEM(r, 1),
                state: if ffi::PyTuple_GET_SIZE(r) > 2 { ffi::PyTuple_GET_ITEM(r, 2) } else { ffi::Py_None() },
                phase: 0,
                i: 0,
                acopy: null_mut(),
                y: null_mut(),
            });
            self.push()?; // `_reconstruct`'s frame
            self.stack.push(Frame { x: incref(x), hold: rv.into_raw(), helper: true, k: K::Recon(Some(rec)) });
            Ok(Began::Pushed)
        }
    }

    /// Enter a native `_deepcopy_*` helper.
    unsafe fn push_helper(&mut self, x: O, c: O) -> R<Began> {
        let st = self.st;
        // SAFETY: as begin.
        unsafe {
            self.push()?;
            let k = if c == st.h_list {
                let y = Own::new(ffi::PyList_New(0))?;
                self.memo.set(x, y.get());
                K::List { y: y.into_raw(), i: 0 }
            } else if c == st.h_dict || c == st.h_frozendict {
                let frozen = c == st.h_frozendict;
                let y = Own::new(ffi::PyDict_New())?;
                if !frozen {
                    self.memo.set(x, y.get());
                }
                K::Dict { y: y.into_raw(), it: DictIt::new(x), key: null_mut(), vcopy: null_mut(), frozen }
            } else if c == st.h_tuple {
                K::Tuple { y: null_mut(), i: 0 }
            } else {
                K::Method
            };
            self.stack.push(Frame { x: incref(x), hold: null_mut(), helper: true, k });
            Ok(Began::Pushed)
        }
    }

    /// Leave a finished frame's helper frame and run deepcopy's epilogue.
    unsafe fn finish(&mut self, f: Frame, y: O) -> R<O> {
        let y = Own(y);
        if f.helper {
            self.pop();
        }
        // SAFETY: live objects.
        unsafe { self.epilogue(f.x, y.get())? };
        self.pop();
        drop(f);
        Ok(y.into_raw())
    }

    /// Advance the top frame; `input` (owned or null) is the copy of the last child.
    unsafe fn step(&mut self, input: O) -> R<Step> {
        let st = self.st;
        let top = self.stack.len() - 1;
        if let K::Recon(slot) = &mut self.stack.as_mut_slice()[top].k {
            let mut rc = slot.take().expect("recon state");
            let x = self.stack.as_mut_slice()[top].x;
            // SAFETY: attached thread.
            let r = unsafe { self.recon(&mut rc, x, input) };
            if let K::Recon(slot) = &mut self.stack.as_mut_slice()[top].k {
                *slot = Some(rc);
            }
            return r;
        }
        let input = Own(input);
        let quick = self.atomic_fast;
        let room = self.budget - self.used > 0;
        // SAFETY: attached thread; frames own what they point to.
        unsafe {
            let f = &mut self.stack.as_mut_slice()[top];
            let x = f.x;
            match &mut f.k {
                K::List { y, i } => {
                    if !input.get().is_null() && ffi::PyList_Append(*y, input.get()) < 0 {
                        return Err(());
                    }
                    // Atomic items inline: `deepcopy(a, memo)` returns `a` once its frame
                    // could be entered (the budget is the same for every item).
                    let inline = quick && room;
                    loop {
                        match list_item(x, *i) {
                            Some(item) => {
                                *i += 1;
                                if inline && is_quick(ty(item)) {
                                    let r = ffi::PyList_Append(*y, item);
                                    decref(item);
                                    if r < 0 {
                                        return Err(());
                                    }
                                    continue;
                                }
                                xdecref(std::mem::replace(&mut f.hold, item));
                                return Ok(Step::Child(item));
                            }
                            None => return Ok(Step::Done(std::mem::replace(y, null_mut()))),
                        }
                    }
                }
                K::Tuple { y, i } => {
                    let n = ffi::PyTuple_GET_SIZE(x);
                    if !input.get().is_null() && ffi::PyList_Append(*y, input.get()) < 0 {
                        return Err(());
                    }
                    let inline = quick && room;
                    while *i < n {
                        let item = ffi::PyTuple_GET_ITEM(x, *i);
                        *i += 1;
                        if inline && is_quick(ty(item)) {
                            if !y.is_null() && ffi::PyList_Append(*y, item) < 0 {
                                return Err(());
                            }
                            continue;
                        }
                        if y.is_null() {
                            *y = nn(ffi::PyList_New(0))?;
                            for j in 0..*i - 1 {
                                if ffi::PyList_Append(*y, ffi::PyTuple_GET_ITEM(x, j)) < 0 {
                                    return Err(());
                                }
                            }
                        }
                        return Ok(Step::Child(item));
                    }
                    // try: return memo[id(x)] except KeyError: pass
                    let ylist = *y;
                    let v = self.memo.lookup(x)?;
                    if !v.is_null() {
                        return Ok(Step::Done(v));
                    }
                    if ylist.is_null() {
                        return Ok(Step::Done(incref(x)));
                    }
                    let same = (0..n).all(|j| ffi::PyTuple_GET_ITEM(x, j) == ffi::PyList_GET_ITEM(ylist, j));
                    if same { Ok(Step::Done(incref(x))) } else { Ok(Step::Done(nn(ffi::PyList_AsTuple(ylist))?)) }
                }
                K::Dict { y, it, key, vcopy, frozen } => {
                    if !input.get().is_null() {
                        if vcopy.is_null() {
                            // The value's copy arrived: now copy the key.
                            *vcopy = input.into_raw();
                            return Ok(Step::Child(*key));
                        }
                        // The key's copy arrived: y[kcopy] = vcopy (a pure key only).
                        let vc = Own(std::mem::replace(vcopy, null_mut()));
                        xdecref(std::mem::replace(key, null_mut()));
                        if !pure_key(input.get()) || ffi::PyDict_SetItem(*y, input.get(), vc.get()) < 0 {
                            return Err(());
                        }
                    }
                    let inline = quick && room;
                    while let Some((k, v)) = it.next_borrowed()? {
                        let own = |o: O| if DictIt::OWNED { o } else { incref(o) };
                        if inline && is_quick(ty(v)) {
                            if is_quick(ty(k)) {
                                let r = ffi::PyDict_SetItem(*y, k, v);
                                if DictIt::OWNED {
                                    decref(k);
                                    decref(v);
                                }
                                if r < 0 {
                                    return Err(());
                                }
                                continue;
                            }
                            *vcopy = own(v);
                            *key = own(k);
                            return Ok(Step::Child(k));
                        }
                        *key = own(k);
                        xdecref(std::mem::replace(&mut f.hold, own(v)));
                        return Ok(Step::Child(v));
                    }
                    if !*frozen {
                        return Ok(Step::Done(std::mem::replace(y, null_mut())));
                    }
                    // frozendict: memo check, then frozendict(y) (keys already checked pure).
                    let ydict = *y;
                    let v = self.memo.lookup(x)?;
                    if !v.is_null() {
                        return Ok(Step::Done(v));
                    }
                    Ok(Step::Done(call1(st.frozendict_type.cast(), ydict)?))
                }
                K::Method => {
                    if input.get().is_null() {
                        return Ok(Step::Child(method_self(x)));
                    }
                    // `type(x)(x.__func__, self_copy)`: `MethodType` checks only.
                    Ok(Step::Done(call2(st.method_type.cast(), method_func(x), input.get())?))
                }
                K::Recon(_) => unreachable!(),
            }
        }
    }

    /// `_reconstruct(x, memo, func, args, state)` with deep=True, natively.
    unsafe fn recon(&mut self, rc: &mut Recon, x: O, input: O) -> R<Step> {
        let st = self.st;
        let input = Own(input);
        // SAFETY: attached thread; `rc` and the frame own what they point to.
        unsafe {
            let n = ffi::PyTuple_GET_SIZE(rc.args);
            match rc.phase {
                0 => {
                    // `args = [deepcopy(arg, memo) for arg in args]` (3.15) / a generator
                    // expression created here and resumed by `func(*args)` per item and once
                    // more to find it exhausted (3.14).
                    rc.acopy = Own::new(ffi::PyList_New(0))?.into_raw();
                    if n > 0 && st.minor < 15 {
                        self.touch()?; // creation
                    }
                    rc.phase = P_ARG;
                    self.recon_next(rc, x)
                }
                P_ARG => {
                    if ffi::PyList_Append(rc.acopy, input.get()) < 0 {
                        return Err(());
                    }
                    if st.minor < 15 {
                        self.pop();
                    }
                    rc.i += 1;
                    self.recon_next(rc, x)
                }
                P_STATE => {
                    apply_state(st, rc.y, input.get())?;
                    Ok(Step::Done(std::mem::replace(&mut rc.y, null_mut())))
                }
                _ => unreachable!(),
            }
        }
    }

    /// The next argument to copy, or `y = func(*args)`, `memo[id(x)] = y` and the state.
    unsafe fn recon_next(&mut self, rc: &mut Recon, x: O) -> R<Step> {
        let st = self.st;
        // SAFETY: as recon.
        unsafe {
            let n = ffi::PyTuple_GET_SIZE(rc.args);
            if rc.i < n {
                if st.minor < 15 {
                    self.push()?; // the generator's frame, resumed around the copy
                }
                return Ok(Step::Child(ffi::PyTuple_GET_ITEM(rc.args, rc.i)));
            }
            if n > 0 && st.minor < 15 {
                self.touch()?; // the resume that finds the generator exhausted
            }
            let a = rc.acopy;
            let y = if rc.func == st.newobj {
                // copyreg.__newobj__(cls) -> object.__new__(cls) (checked eligible).
                if !self.helper(H_NEWOBJ) || ffi::PyList_GET_SIZE(a) != 1 {
                    return Err(());
                }
                self.touch()?;
                object_new_alloc(st, ffi::PyList_GET_ITEM(a, 0)).unwrap_or(Err(()))?
            } else if rc.func == (&raw mut ffi::PySet_Type).cast() || rc.func == (&raw mut ffi::PyFrozenSet_Type).cast()
            {
                // set/frozenset(list_copy): hashing the copies must be pure.
                if ffi::PyList_GET_SIZE(a) != 1 {
                    return Err(());
                }
                let l = ffi::PyList_GET_ITEM(a, 0);
                if ty(l) != &raw mut ffi::PyList_Type {
                    return Err(());
                }
                for j in 0..ffi::PyList_GET_SIZE(l) {
                    if !pure_key(ffi::PyList_GET_ITEM(l, j)) {
                        return Err(());
                    }
                }
                call1(rc.func, l)?
            } else {
                // A recognised `_datetime` type (exact, `datetime_reduce`): its C constructor
                // takes the copies (bytes, ints, a tzinfo copied natively, a name).
                let t = Own::new(ffi::PyList_AsTuple(a))?;
                nn(ffi::PyObject_Call(rc.func, t.get(), null_mut()))?
            };
            rc.y = y;
            self.memo.set(x, y);
            if rc.state == ffi::Py_None() {
                return Ok(Step::Done(std::mem::replace(&mut rc.y, null_mut())));
            }
            rc.phase = P_STATE;
            Ok(Step::Child(rc.state))
        }
    }
}

/// The state branch of `_reconstruct` for a plain object whose class has no
/// `__setstate__`, a standard `__dict__` and member-descriptor slots (checked eligible):
/// `y.__dict__.update(state)`, then `setattr(y, k, v)` per slot.
pub unsafe fn apply_state(st: &St, y: O, sc: O) -> R<()> {
    // SAFETY: live objects; C-level operations only.
    unsafe {
        let (state, slots) = if ty(sc) == &raw mut ffi::PyTuple_Type && ffi::PyTuple_GET_SIZE(sc) == 2 {
            (ffi::PyTuple_GET_ITEM(sc, 0), ffi::PyTuple_GET_ITEM(sc, 1))
        } else {
            (sc, ffi::Py_None())
        };
        if state != ffi::Py_None() {
            if ty(state) != &raw mut ffi::PyDict_Type {
                return Err(());
            }
            let dd = Own::new(ffi::PyObject_GetAttr(y, st.s_dict))?;
            if ty(dd.get()) != &raw mut ffi::PyDict_Type || ffi::PyDict_Merge(dd.get(), state, 1) < 0 {
                return Err(());
            }
        }
        if slots != ffi::Py_None() {
            if ty(slots) != &raw mut ffi::PyDict_Type {
                return Err(());
            }
            let mut pos = 0;
            let mut k: O = null_mut();
            let mut v: O = null_mut();
            while ffi::PyDict_Next(slots, &mut pos, &mut k, &mut v) != 0 {
                if ffi::PyObject_SetAttr(y, k, v) < 0 {
                    return Err(());
                }
            }
        }
        Ok(())
    }
}
