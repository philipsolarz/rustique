//! `copy.deepcopy`, with the whole recursion inside the kernel.
//!
//! The engine executes `Lib/copy.py`'s algorithm step for step — the same memo writes,
//! keep-alive appends, iteration protocol, evaluation order (`y[deepcopy(k)] =
//! deepcopy(v)` copies the value first), hook calls and Python-frame depth — with an
//! explicit work stack instead of C recursion (CPython's own frames for this recursion live
//! on the heap, so a deep structure must not need a deep C stack here either).
//!
//! Native steps: atomic types, memo hits, `list`/`tuple`/`dict`/`frozendict`/bound methods
//! through the original `_deepcopy_*` helpers, `issubclass(cls, type)`, `__deepcopy__`
//! methods that are `return self`, and `_reconstruct` for reduce values that are exact
//! tuples. Everything a user can hook (`__deepcopy__`, registered copiers, reductors,
//! `__reduce_ex__`, `__setstate__`, constructors, `__getattr__`, `__hash__`, ...) is called
//! through the same C-API operation CPython's bytecode uses, at the same point.

use std::ptr::null_mut;

use pyo3::ffi;

use super::memo::{Look, Memo, Native};
use super::rt::*;
use super::shallow::{call_func, set_state};

enum K {
    /// `_deepcopy_list`: `y` (strong) built by appends; next index `i`.
    List { y: O, i: ffi::Py_ssize_t },
    /// `_deepcopy_tuple`: the list comprehension of copies.
    Tuple { y: O, i: ffi::Py_ssize_t },
    /// `_deepcopy_dict` / `_deepcopy_frozendict`: items iterator; `key` waits (strong) while
    /// its value is copied, then `vcopy` (strong) waits while the key is copied.
    Dict { y: O, it: DictIt, key: O, vcopy: O, frozen: bool },
    /// `_deepcopy_method`: waiting for the copy of `__self__`.
    Method,
    /// `_reconstruct(x, memo, *rv)`.
    Recon(Option<Box<Recon>>),
}

struct Frame {
    x: O,    // strong
    d: O,    // `id(x)` int in dict mode (strong) or null
    hold: O, // strong or null: the reduce value, kept until this deepcopy call ends
    helper: bool,
    k: K,
}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: owned references.
        unsafe {
            match &mut self.k {
                K::List { y, .. } | K::Tuple { y, .. } => xdecref(*y),
                K::Dict { y, it, key, vcopy, .. } => {
                    xdecref(*vcopy);
                    xdecref(*key);
                    xdecref(*y);
                    let _ = it;
                }
                K::Method => {}
                K::Recon(_) => {}
            }
            xdecref(self.hold);
            xdecref(self.d);
            decref(self.x);
        }
    }
}

/// `_reconstruct` state (deep).
struct Recon {
    func: O,
    args: O,
    state: O,
    listiter: O,
    dictiter: O, // borrowed from the reduce value (kept by Frame::hold)
    phase: u8,
    acopies: O, // strong: copied args (list)
    ait: O,     // strong: iterator over args / listiter / dictiter
    aidx: ffi::Py_ssize_t,
    y: O,         // strong
    cur: O,       // strong: the item being copied
    k: O,         // strong: dictiter key copy, waiting for the value
    v: O,         // strong: dictiter value (source), waiting
    in_gen: bool, // 3.14: the generator expression frame is entered
}

impl Drop for Recon {
    fn drop(&mut self) {
        // SAFETY: owned references.
        unsafe {
            xdecref(self.v);
            xdecref(self.k);
            xdecref(self.cur);
            xdecref(self.y);
            xdecref(self.ait);
            xdecref(self.acopies);
        }
    }
}

const P_ARGS: u8 = 1;
const P_ARGS_DONE: u8 = 7;
const P_STATE: u8 = 2;
const P_LIST: u8 = 4;
const P_DICT_K: u8 = 5;
const P_DICT_V: u8 = 6;

enum Began {
    Ready(O),
    Pushed,
}

enum Step {
    /// Copy this object next (borrowed: kept alive by the frame).
    Child(O),
    Done(O),
    /// Run the top frame again without input.
    Again,
}

pub struct Engine {
    pub st: &'static St,
    pub depth: Depth,
    pub memo: Memo,
    stack: SVec<Frame, 8>,
    /// Helpers verified intact (bit i) / modified (bit 16 + i) in this call.
    checked: u32,
    atomic_fast: bool,
}

impl Engine {
    /// `memo`: null (`deepcopy(x)`) or the caller's memo object (borrowed).
    #[inline(always)]
    pub unsafe fn new(st: &'static St, memo: O, atomic_fast: bool) -> Engine {
        Engine {
            st,
            // SAFETY: attached thread.
            depth: unsafe { Depth::new() },
            // SAFETY: live dict.
            memo: if memo.is_null() { Memo::Native(Native::new()) } else { Memo::Dict(unsafe { incref(memo) }) },
            stack: SVec::new(),
            checked: 0,
            atomic_fast,
        }
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

    /// `cls in _atomic_types`.
    #[inline(always)]
    unsafe fn atomic(&self, cls: *mut ffi::PyTypeObject) -> R<bool> {
        let st = self.st;
        if self.atomic_fast {
            let a = &st.atomic_types;
            // Most frequent first.
            if cls == a[0] || cls == a[1] || cls == a[2] || cls == a[3] || cls == a[4] {
                return Ok(true);
            }
            // SAFETY: live type object.
            unsafe {
                if meta_hash(cls.cast()) == default_type_hash() {
                    // A pointer-hashed type: the set lookup is pure, decide by identity.
                    return Ok(a[5..].contains(&cls));
                }
            }
        }
        // SAFETY: live set and type.
        match unsafe { ffi::PySet_Contains(st.atomic, cls.cast()) } {
            r if r < 0 => Err(()),
            r => Ok(r != 0),
        }
    }

    pub unsafe fn run(&mut self, root: O) -> R<O> {
        // SAFETY: attached thread; every object handled is kept alive by a frame.
        unsafe {
            let mut input: O = match self.begin(root, true) {
                Ok(Began::Ready(y)) => return Ok(y),
                Ok(Began::Pushed) => null_mut(),
                Err(()) => return self.fail(),
            };
            loop {
                match self.step(input) {
                    Err(()) => return self.fail(),
                    Ok(Step::Again) => input = null_mut(),
                    Ok(Step::Child(c)) => match self.begin(c, false) {
                        Ok(Began::Ready(y)) => input = y,
                        Ok(Began::Pushed) => input = null_mut(),
                        Err(()) => return self.fail(),
                    },
                    Ok(Step::Done(y)) => {
                        let f = self.stack.pop().expect("frame");
                        match self.finish(f, y) {
                            Ok(y) => {
                                if self.stack.is_empty() {
                                    return Ok(y);
                                }
                                input = y;
                            }
                            Err(()) => return self.fail(),
                        }
                    }
                }
            }
        }
    }

    #[cold]
    fn fail(&mut self) -> R<O> {
        while let Some(f) = self.stack.pop() {
            drop(f);
        }
        self.depth.unwind();
        Err(())
    }

    /// The end of `deepcopy(x)`: `if y is not x: memo[d] = y; _keep_alive(x, memo)`.
    #[inline(always)]
    unsafe fn epilogue(&mut self, x: O, d: O, y: O) -> R<()> {
        if y != x {
            // SAFETY: live objects.
            unsafe {
                self.memo.set(x, d, y)?;
                self.keep_alive(x)?;
            }
        }
        Ok(())
    }

    #[inline(always)]
    unsafe fn keep_alive(&mut self, x: O) -> R<()> {
        // SAFETY: live objects.
        unsafe {
            if self.helper(H_KEEP_ALIVE) {
                self.depth.touch()?;
                self.memo.keep_alive(self.st, x)
            } else {
                let m = self.memo.dict()?;
                let r = call2(self.st.keep_alive, x, m)?;
                decref(r);
                Ok(())
            }
        }
    }

    /// A deepcopy call that produced `y` without pushing a work frame.
    #[inline(always)]
    unsafe fn ready(&mut self, x: O, d: O, y: O) -> R<Began> {
        let y = Own(y);
        let d = Own(d);
        // SAFETY: live objects.
        unsafe { self.epilogue(x, d.get(), y.get())? };
        self.depth.pop();
        Ok(Began::Ready(y.into_raw()))
    }

    /// The start of `deepcopy(x, memo)`; `x` borrowed.
    unsafe fn begin(&mut self, x: O, root: bool) -> R<Began> {
        let st = self.st;
        // SAFETY: attached thread; x alive for the duration (caller's frame holds it).
        unsafe {
            self.depth.push()?;
            let cls = ty(x);
            match self.atomic(cls) {
                Ok(true) => {
                    self.depth.pop();
                    return Ok(Began::Ready(incref(x)));
                }
                Ok(false) => {}
                Err(()) => {
                    self.depth.pop();
                    return Err(());
                }
            }
            let d = if root && self.memo.is_native() {
                null_mut()
            } else {
                match self.memo.lookup(x, st.minor, st) {
                    Ok(Look::Hit(y)) => {
                        self.depth.pop();
                        return Ok(Began::Ready(y));
                    }
                    Ok(Look::Miss(d)) => d,
                    Err(()) => {
                        self.depth.pop();
                        return Err(());
                    }
                }
            };
            let r = self.dispatch(x, cls, d);
            if r.is_err() {
                // `d` was consumed by dispatch on every path.
                self.depth.pop();
            }
            r
        }
    }

    /// From `copier = _deepcopy_dispatch.get(cls)` on. Takes ownership of `d`.
    unsafe fn dispatch(&mut self, x: O, cls: *mut ffi::PyTypeObject, d: O) -> R<Began> {
        let st = self.st;
        // SAFETY: as begin.
        unsafe {
            let d = Own(d);
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
                if native {
                    return self.push_helper(x, d.into_raw(), c);
                }
                let m = self.memo.dict()?;
                let y = call2(c, x, m)?;
                return self.ready(x, d.into_raw(), y);
            }
            if ffi::PyType_IsSubtype(cls, &raw mut ffi::PyType_Type) != 0 {
                return self.ready(x, d.into_raw(), incref(x));
            }
            let cp = Own(getattr_opt(x, st.s_deepcopy)?);
            if !cp.get().is_null() && cp.get() != ffi::Py_None() {
                let c = cp.get();
                if ty(c) == &raw mut PyMethod_Type && is_return_self(method_func(c), 2) {
                    // `def __deepcopy__(self, memo): return self` (Enum, ...): its frame only.
                    self.depth.touch()?;
                    return self.ready(x, d.into_raw(), incref(method_self(c)));
                }
                let m = self.memo.dict()?;
                let y = call1(c, m)?;
                return self.ready(x, d.into_raw(), y);
            }
            let rv = Own::new(self.reduce(x, cls)?)?;
            if is_str_instance(rv.get())? {
                return self.ready(x, d.into_raw(), incref(x));
            }
            self.reconstruct(x, d.into_raw(), rv.into_raw())
        }
    }

    /// `rv` from `dispatch_table` / `__reduce_ex__(4)` / `__reduce__()`.
    unsafe fn reduce(&mut self, x: O, cls: *mut ffi::PyTypeObject) -> R<O> {
        let st = self.st;
        // SAFETY: as begin.
        unsafe {
            let reductor = Own(dict_get(st.dispatch_table, cls.cast())?);
            if !reductor.get().is_null() && truth(reductor.get())? {
                return call1(reductor.get(), x);
            }
            drop(reductor);
            if (cls == &raw mut ffi::PySet_Type || cls == &raw mut ffi::PyFrozenSet_Type) && self.helper(H_SLOTNAMES) {
                // `object.__reduce_ex__` -> `set.__reduce__` (exact set/frozenset):
                // `(type(x), (list(x),), state)`, where `state` is `None` after
                // `_PyType_GetSlotNames` called `copyreg._slotnames(cls)` (which returns [] and
                // fails to cache it on the static type): only that frame's depth check remains.
                let keys = Own::new(ffi::PySequence_List(x))?;
                self.depth.touch()?;
                let args = Own::new(ffi::PyTuple_Pack(1, keys.get()))?;
                return nn(ffi::PyTuple_Pack(3, cls.cast::<ffi::PyObject>(), args.get(), ffi::Py_None()));
            }
            if let Some(rv) = super::reduce::plain_reduce(st, x, cls)? {
                return Ok(rv);
            }
            let r = Own(getattr_opt(x, st.s_reduce_ex)?);
            if !r.get().is_null() && r.get() != ffi::Py_None() {
                return call1(r.get(), st.int4);
            }
            let r = Own(getattr_opt(x, st.s_reduce)?);
            if !r.get().is_null() && truth(r.get())? {
                return call0(r.get());
            }
            raise_copy_error(st, c"un(deep)copyable object of type %S".as_ptr(), cls)
        }
    }

    /// `y = _reconstruct(x, memo, *rv)`. Takes ownership of `d` and `rv`.
    unsafe fn reconstruct(&mut self, x: O, d: O, rv: O) -> R<Began> {
        let st = self.st;
        // SAFETY: as begin.
        unsafe {
            let d = Own(d);
            let rv = Own(rv);
            let r = rv.get();
            let n = if ty(r) == &raw mut ffi::PyTuple_Type { ffi::PyTuple_GET_SIZE(r) } else { -1 };
            if !(2..=5).contains(&n) || !self.helper(H_RECONSTRUCT) {
                // CPython's own `_reconstruct`, called as `deepcopy` calls it.
                let m = self.memo.dict()?;
                let args = Own::new(ffi::PyList_New(0))?;
                if ffi::PyList_Append(args.get(), x) < 0 || ffi::PyList_Append(args.get(), m) < 0 {
                    return Err(());
                }
                if ffi::PyList_Extend(args.get(), r) < 0 {
                    let t = ty(r);
                    if ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError) != 0
                        && (*t).tp_iter.is_none()
                        && ffi::PySequence_Check(r) == 0
                    {
                        ffi::PyErr_Clear();
                        ffi::PyErr_Format(
                            ffi::PyExc_TypeError,
                            c"Value after * must be an iterable, not %.200s".as_ptr(),
                            (*t).tp_name,
                        );
                    }
                    return Err(());
                }
                let t = Own::new(ffi::PyList_AsTuple(args.get()))?;
                let y = nn(ffi::PyObject_Call(st.reconstruct, t.get(), null_mut()))?;
                return self.ready(x, d.into_raw(), y);
            }
            let item = |i: ffi::Py_ssize_t| if i < n { ffi::PyTuple_GET_ITEM(r, i) } else { ffi::Py_None() };
            let rec = Box::new(Recon {
                func: item(0),
                args: item(1),
                state: item(2),
                listiter: item(3),
                dictiter: item(4),
                phase: 0,
                acopies: null_mut(),
                ait: null_mut(),
                aidx: 0,
                y: null_mut(),
                cur: null_mut(),
                k: null_mut(),
                v: null_mut(),
                in_gen: false,
            });
            // The `_reconstruct` frame.
            self.depth.push()?;
            self.stack.push(Frame {
                x: incref(x),
                d: d.into_raw(),
                hold: rv.into_raw(),
                helper: true,
                k: K::Recon(Some(rec)),
            });
            Ok(Began::Pushed)
        }
    }

    /// Enter a native `_deepcopy_*` helper. Takes ownership of `d`.
    unsafe fn push_helper(&mut self, x: O, d: O, c: O) -> R<Began> {
        let st = self.st;
        // SAFETY: as begin.
        unsafe {
            let d = Own(d);
            self.depth.push()?;
            let k = if c == st.h_list {
                let y = match Own::new(ffi::PyList_New(0)) {
                    Ok(y) => y,
                    Err(()) => {
                        self.depth.pop();
                        return Err(());
                    }
                };
                if self.memo.set(x, d.get(), y.get()).is_err() {
                    self.depth.pop();
                    return Err(());
                }
                K::List { y: y.into_raw(), i: 0 }
            } else if c == st.h_dict || c == st.h_frozendict {
                let frozen = c == st.h_frozendict;
                let y = match Own::new(ffi::PyDict_New()) {
                    Ok(y) => y,
                    Err(()) => {
                        self.depth.pop();
                        return Err(());
                    }
                };
                if !frozen && self.memo.set(x, d.get(), y.get()).is_err() {
                    self.depth.pop();
                    return Err(());
                }
                K::Dict { y: y.into_raw(), it: DictIt::new(x), key: null_mut(), vcopy: null_mut(), frozen }
            } else if c == st.h_tuple {
                // The comprehension's list is only built once a copy can differ.
                K::Tuple { y: null_mut(), i: 0 }
            } else {
                K::Method
            };
            self.stack.push(Frame { x: incref(x), d: d.into_raw(), hold: null_mut(), helper: true, k });
            Ok(Began::Pushed)
        }
    }

    /// Pop a finished frame's helper frame and run deepcopy's epilogue.
    unsafe fn finish(&mut self, f: Frame, y: O) -> R<O> {
        let y = Own(y);
        if f.helper {
            self.depth.pop();
        }
        // SAFETY: live objects.
        unsafe { self.epilogue(f.x, f.d, y.get())? };
        self.depth.pop();
        drop(f);
        Ok(y.into_raw())
    }

    /// Advance the top frame; `input` (owned or null) is the copy of the last child.
    unsafe fn step(&mut self, input: O) -> R<Step> {
        let st = self.st;
        let top = self.stack.len() - 1;
        if let K::Recon(slot) = &mut self.stack.as_mut_slice()[top].k {
            let mut rc = slot.take().expect("recon state");
            let (x, d) = (self.stack.as_mut_slice()[top].x, self.stack.as_mut_slice()[top].d);
            // SAFETY: attached thread.
            let r = unsafe { self.recon(&mut rc, x, d, input) };
            if let K::Recon(slot) = &mut self.stack.as_mut_slice()[top].k {
                *slot = Some(rc);
            }
            return r;
        }
        let input = Own(input);
        let quick = self.atomic_fast;
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
                    let inline = quick && *self.depth.rem > 0;
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
                                // The item stays alive in a temporary frame slot: `hold`.
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
                    let inline = quick && *self.depth.rem > 0;
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
                            // The comprehension's list, holding the copies so far (the
                            // atomic items themselves).
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
                    let (ylist, d) = (*y, f.d);
                    match &mut self.memo {
                        Memo::Native(m) => {
                            if let Some(v) = m.get(x) {
                                return Ok(Step::Done(incref(v)));
                            }
                        }
                        Memo::Dict(m) => {
                            let key =
                                if d.is_null() { Own::new(ffi::PyLong_FromVoidPtr(x.cast()))? } else { Own(incref(d)) };
                            let v = ffi::PyObject_GetItem(*m, key.get());
                            if !v.is_null() {
                                return Ok(Step::Done(v));
                            }
                            if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) == 0 {
                                return Err(());
                            }
                            ffi::PyErr_Clear();
                        }
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
                        // The key's copy arrived: y[kcopy] = vcopy.
                        let vc = Own(std::mem::replace(vcopy, null_mut()));
                        xdecref(std::mem::replace(key, null_mut()));
                        if ffi::PyDict_SetItem(*y, input.get(), vc.get()) < 0 {
                            return Err(());
                        }
                    }
                    let inline = quick && *self.depth.rem > 0;
                    while let Some((k, v)) = it.next_borrowed()? {
                        // Strong references for whatever outlives this iteration.
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
                            // The value is its own copy; the key needs the full path.
                            *vcopy = own(v);
                            *key = own(k);
                            return Ok(Step::Child(k));
                        }
                        *key = own(k);
                        xdecref(std::mem::replace(&mut f.hold, own(v)));
                        return Ok(Step::Child(v));
                    }
                    {
                        {
                            if !*frozen {
                                return Ok(Step::Done(std::mem::replace(y, null_mut())));
                            }
                            // frozendict: memo check, then frozendict(y).
                            let ydict = *y;
                            let d = f.d;
                            match &mut self.memo {
                                Memo::Native(n) => {
                                    if let Some(v) = n.get(x) {
                                        return Ok(Step::Done(incref(v)));
                                    }
                                }
                                Memo::Dict(m) => {
                                    let key = if d.is_null() {
                                        Own::new(ffi::PyLong_FromVoidPtr(x.cast()))?
                                    } else {
                                        Own(incref(d))
                                    };
                                    let v = ffi::PyObject_GetItem(*m, key.get());
                                    if !v.is_null() {
                                        return Ok(Step::Done(v));
                                    }
                                    if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) == 0 {
                                        return Err(());
                                    }
                                    ffi::PyErr_Clear();
                                }
                            }
                            Ok(Step::Done(call1(st.frozendict_type.cast(), ydict)?))
                        }
                    }
                }
                K::Method => {
                    if input.get().is_null() {
                        return Ok(Step::Child(method_self(x)));
                    }
                    let y = call2(st.method_type.cast(), method_func(x), input.get())?;
                    Ok(Step::Done(y))
                }
                K::Recon(_) => unreachable!(),
            }
        }
    }

    /// `_reconstruct(x, memo, func, args, state, listiter, dictiter)` with deep=True.
    /// `rc` is moved out of its frame while it runs.
    unsafe fn recon(&mut self, rc: &mut Recon, x: O, d: O, input: O) -> R<Step> {
        let st = self.st;
        let input = Own(input);
        // SAFETY: attached thread; `rc` and the frame own what they point to.
        unsafe {
            match rc.phase {
                0 => {
                    // if deep and args: args = [deepcopy(arg, memo) for arg in args]
                    if truth(rc.args)? {
                        rc.acopies = nn(ffi::PyList_New(0))?;
                        if ty(rc.args) != &raw mut ffi::PyTuple_Type {
                            rc.ait = nn(ffi::PyObject_GetIter(rc.args))?;
                        }
                        if st.minor < 15 {
                            // 3.14: a generator expression, created here and resumed by
                            // `func(*args)` once per argument (and once more to finish).
                            self.depth.touch()?;
                        }
                        rc.phase = P_ARGS;
                        self.recon_next_arg(rc)
                    } else {
                        self.recon_call(rc, x, d)
                    }
                }
                P_ARGS => {
                    if ffi::PyList_Append(rc.acopies, input.get()) < 0 {
                        return Err(());
                    }
                    xdecref(std::mem::replace(&mut rc.cur, null_mut()));
                    self.recon_next_arg(rc)
                }
                P_ARGS_DONE => self.recon_call(rc, x, d),
                P_STATE => {
                    set_state(self.st, rc.y, input.get())?;
                    rc.phase = P_LIST;
                    self.recon_items(rc, null_mut())
                }
                _ => self.recon_items(rc, input.into_raw()),
            }
        }
    }

    unsafe fn recon_next_arg(&mut self, rc: &mut Recon) -> R<Step> {
        let minor = self.st.minor;
        // SAFETY: live args / iterator.
        unsafe {
            if rc.in_gen {
                self.depth.pop();
                rc.in_gen = false;
            }
            if minor < 15 {
                self.depth.push()?;
                rc.in_gen = true;
            }
            let next = if rc.ait.is_null() {
                if rc.aidx < ffi::PyTuple_GET_SIZE(rc.args) {
                    let a = incref(ffi::PyTuple_GET_ITEM(rc.args, rc.aidx));
                    rc.aidx += 1;
                    Some(a)
                } else {
                    None
                }
            } else {
                iter_next(rc.ait)?
            };
            match next {
                Some(a) => {
                    rc.cur = a;
                    Ok(Step::Child(a))
                }
                None => {
                    if rc.in_gen {
                        self.depth.pop();
                        rc.in_gen = false;
                    }
                    xdecref(std::mem::replace(&mut rc.ait, null_mut()));
                    rc.phase = P_ARGS_DONE;
                    Ok(Step::Again)
                }
            }
        }
    }

    /// `y = func(*args)`, `memo[id(x)] = y`, then the state.
    unsafe fn recon_call(&mut self, rc: &mut Recon, x: O, d: O) -> R<Step> {
        let st = self.st;
        // SAFETY: live objects.
        unsafe {
            let callargs = if rc.acopies.is_null() { rc.args } else { rc.acopies };
            let newobj_ok = rc.func == st.newobj && self.helper(H_NEWOBJ);
            let y = call_func(st, &mut self.depth, rc.func, callargs, newobj_ok)?;
            rc.y = y;
            self.memo.set(x, d, y)?;
            if rc.state != ffi::Py_None() {
                rc.phase = P_STATE;
                return Ok(Step::Child(rc.state));
            }
            rc.phase = P_LIST;
            self.recon_items(rc, null_mut())
        }
    }

    /// The `listiter` / `dictiter` loops of `_reconstruct`.
    unsafe fn recon_items(&mut self, rc: &mut Recon, input: O) -> R<Step> {
        let st = self.st;
        let input = Own(input);
        // SAFETY: live objects.
        unsafe {
            match rc.phase {
                P_LIST => {
                    if !input.get().is_null() {
                        // y.append(item)
                        if ty(rc.y) == &raw mut ffi::PyList_Type {
                            if ffi::PyList_Append(rc.y, input.get()) < 0 {
                                return Err(());
                            }
                        } else {
                            decref(call_method1(rc.y, st.s_append, input.get())?);
                        }
                        xdecref(std::mem::replace(&mut rc.cur, null_mut()));
                    } else if rc.listiter != ffi::Py_None() {
                        rc.ait = nn(ffi::PyObject_GetIter(rc.listiter))?;
                    } else {
                        return Self::recon_start_dict(rc);
                    }
                    match iter_next(rc.ait)? {
                        Some(item) => {
                            rc.cur = item;
                            Ok(Step::Child(item))
                        }
                        None => {
                            xdecref(std::mem::replace(&mut rc.ait, null_mut()));
                            Self::recon_start_dict(rc)
                        }
                    }
                }
                P_DICT_K => {
                    // The key's copy arrived; copy the value.
                    rc.k = input.into_raw();
                    rc.phase = P_DICT_V;
                    Ok(Step::Child(rc.v))
                }
                P_DICT_V => {
                    let k = Own(std::mem::replace(&mut rc.k, null_mut()));
                    if ffi::PyObject_SetItem(rc.y, k.get(), input.get()) < 0 {
                        return Err(());
                    }
                    xdecref(std::mem::replace(&mut rc.v, null_mut()));
                    xdecref(std::mem::replace(&mut rc.cur, null_mut()));
                    rc.phase = P_DICT_K;
                    Self::recon_next_dict(rc)
                }
                _ => unreachable!(),
            }
        }
    }

    unsafe fn recon_start_dict(rc: &mut Recon) -> R<Step> {
        rc.phase = P_DICT_K;
        // SAFETY: immortal singleton.
        if rc.dictiter == unsafe { ffi::Py_None() } {
            return Ok(Step::Done(std::mem::replace(&mut rc.y, null_mut())));
        }
        // SAFETY: live object.
        rc.ait = nn(unsafe { ffi::PyObject_GetIter(rc.dictiter) })?;
        // SAFETY: as above.
        unsafe { Self::recon_next_dict(rc) }
    }

    unsafe fn recon_next_dict(rc: &mut Recon) -> R<Step> {
        // SAFETY: live iterator.
        unsafe {
            match iter_next(rc.ait)? {
                Some(pair) => {
                    let pair = Own(pair);
                    let (k, v) = unpack2(pair.get())?;
                    rc.cur = k.into_raw();
                    rc.v = v.into_raw();
                    Ok(Step::Child(rc.cur))
                }
                None => {
                    xdecref(std::mem::replace(&mut rc.ait, null_mut()));
                    Ok(Step::Done(std::mem::replace(&mut rc.y, null_mut())))
                }
            }
        }
    }
}
