/* Rustique interp tier: opcode handlers generated from one CPython patch release.
 *
 * Compiled once per handler set (build.rs): -DRQ_SET=<v3_15_1_gil|...> with the include
 * paths of gen/<release>/ (that release's headers, Python/ceval.h, Python/ceval_macros.h,
 * the generated rq_cases.c.h / rq_targets.h, and the pyconfig.h of the verified builds of
 * that configuration). The result is a set of tail-calling handlers with CPython's own ABI;
 * the Rust side (src/lib.rs) verifies the running binary and writes chosen handlers into the
 * core's dispatch table (instruction_funcptr_handler_table). A swapped handler runs with the
 * core's frame, stack and thread state and dispatches onward through the table it was given,
 * back into the core's handlers.
 *
 * Adapted from CPython's Modules/_testinternalcapi/interpreter.c (PSF licence, gen/LICENSE),
 * with three differences, each making the copy behave like the core's own handlers:
 *  - DISPATCH_TABLE / TRACING_DISPATCH_TABLE name the core's tables, not this file's;
 *  - IS_PEP523_HOOKED keeps the core's definition (the handlers run inside the core's loop);
 *  - nothing reachable from a swapped handler may enter tier 2: the JIT entry and the trace
 *    compiler are declared with __attribute__((error)), so a reachable use fails the build. */
#define PY_SSIZE_T_CLEAN
#ifndef Py_BUILD_CORE
#  define Py_BUILD_CORE
#endif
#ifndef Py_BUILD_CORE_MODULE
#  define Py_BUILD_CORE_MODULE
#endif

/* Every symbol this file references in the interpreter is weak (gen/<release>/rq_weak.h):
 * the extension must load on every CPython, including builds where the tier refuses. */
#include "rq_weak.h"

#include "ceval.h"
#include "ceval_macros.h"

#if !_Py_TAIL_CALL_INTERP
#  error "rq_handlers.c must be compiled with the verified builds' pyconfig.h (tail-calling)"
#endif
#ifdef Py_STATS
#  error "Py_STATS builds are not supported"
#endif

#include "rq_api.h"
#include "rq_common.h"

#undef DISPATCH_TABLE
#define DISPATCH_TABLE rq_core_handler_table
#undef TRACING_DISPATCH_TABLE
#define TRACING_DISPATCH_TABLE rq_core_tracing_table

#ifdef RQ_WEAK_SCAN
/* tools/gen.py: compile every handler of the file to list the symbols it can reference. */
#  ifdef _Py_TIER2
static int stop_tracing_and_jit(PyThreadState *tstate, _PyInterpreterFrame *frame)
{ (void)tstate; (void)frame; abort(); }
#  endif
static _Py_CODEUNIT *
rq_unreachable_jit_entry(struct _PyExecutorObject *exec, _PyInterpreterFrame *frame,
                         _PyStackRef *stack_pointer, PyThreadState *tstate)
{ (void)exec; (void)frame; (void)stack_pointer; (void)tstate; abort(); }
#else
#  ifdef _Py_TIER2
static int stop_tracing_and_jit(PyThreadState *tstate, _PyInterpreterFrame *frame)
    __attribute__((error("rustique interp: a swapped handler reaches the trace compiler")));
#  endif
static _Py_CODEUNIT *rq_unreachable_jit_entry(struct _PyExecutorObject *exec,
                                              _PyInterpreterFrame *frame,
                                              _PyStackRef *stack_pointer,
                                              PyThreadState *tstate)
    __attribute__((error("rustique interp: a swapped handler reaches an executor")));
#endif
#define _Py_jit_entry rq_unreachable_jit_entry

#include "rq_inline.h"
#include "rq_isinstance.h"
#include "rq_attrcache.h"
#include "rq_calls.h"

#include "rq_targets.h"
#include "rq_cases.c.h"

#ifdef RQ_WEAK_SCAN
void *rq_weak_scan_tables[] = {instruction_funcptr_handler_table, instruction_funcptr_tracing_table};
#endif

/* ---- The set's interface (rq_api.h) ---------------------------------------------------- */

#include "rq_groups.h"   /* RQ_SWAPPABLE(X): X(opcode, group) for every handler we may swap */

static void *
rq_handler(int opcode)
{
    switch (opcode) {
#define RQ_CASE(op, group) case op: return (void *)_TAIL_CALL_##op;
    RQ_SWAPPABLE(RQ_CASE)
#undef RQ_CASE
    default:
        return NULL;
    }
}

static int
rq_group_of(int opcode)
{
    switch (opcode) {
#define RQ_CASE(op, group) case op: return group;
    RQ_SWAPPABLE(RQ_CASE)
#undef RQ_CASE
    default:
        return -1;
    }
}

static const char *
rq_opname(int opcode)
{
    switch (opcode) {
#define RQ_CASE(op, group) case op: return #op;
    RQ_SWAPPABLE(RQ_CASE)
#undef RQ_CASE
    default:
        return NULL;
    }
}

static int rq_initialized;

static int
rq_init(const rq_init_args *a)
{
    if (rq_initialized) {
        return 0;
    }
    rq_core_handler_table = (py_tail_call_funcptr *)a->core_handler_table;
    rq_core_tracing_table = (py_tail_call_funcptr *)a->core_tracing_table;
    rq_on_suspend = a->on_suspend;
#ifdef _Py_TIER2
    if (rq_core_tracing_table == NULL) {
        return -1;
    }
#endif
    rq_main_interp = PyInterpreterState_Main();
    /* Static types: these live as long as the process (main interpreter's dicts). */
    rq_object_class_descr = _PyType_Lookup(&PyBaseObject_Type, &_Py_ID(__class__));
    rq_type_instancecheck = _PyType_Lookup(&PyType_Type, &_Py_ID(__instancecheck__));
    rq_object_setattr = _PyType_Lookup(&PyBaseObject_Type, &_Py_ID(__setattr__));
    if (rq_object_class_descr == NULL || rq_type_instancecheck == NULL
        || rq_object_setattr == NULL || !PyObject_TypeCheck(rq_object_setattr, &PyWrapperDescr_Type)
        || ((PyWrapperDescrObject *)rq_object_setattr)->d_wrapped != (void *)PyObject_GenericSetAttr) {
        return -1;
    }
    rq_initialized = 1;
    return 0;
}

/* ABI self-test: call the core's NOP handler with sentinel arguments through a private
 * dispatch table whose entry for the next opcode is a probe of ours. The core's handler
 * stores frame->instr_ptr, advances next_instr, decodes the next code unit and tail-calls the
 * table entry; the probe checks that every argument arrived where this compiler expects it,
 * in both directions (we call the core with preserve_none, the core calls us with
 * preserve_none). Touches only the fake frame below. */
static struct {
    int calls;
    _PyInterpreterFrame *frame;
    _PyStackRef *sp;
    PyThreadState *tstate;
    _Py_CODEUNIT *next_instr;
    const void *table;
    int oparg;
} rq_abi_seen;

static PyObject *Py_PRESERVE_NONE_CC
rq_abi_probe(TAIL_CALL_PARAMS)
{
    rq_abi_seen.calls++;
    rq_abi_seen.frame = frame;
    rq_abi_seen.sp = stack_pointer;
    rq_abi_seen.tstate = tstate;
    rq_abi_seen.next_instr = next_instr;
    rq_abi_seen.table = instruction_funcptr_table;
    rq_abi_seen.oparg = oparg;
    return (PyObject *)&rq_abi_seen;
}

static PyObject *Py_PRESERVE_NONE_CC
rq_abi_wrong(TAIL_CALL_PARAMS)
{
    (void)frame; (void)stack_pointer; (void)tstate; (void)next_instr;
    (void)instruction_funcptr_table; (void)oparg;
    return NULL;
}

static int
rq_abi_selftest(void *core_nop)
{
    static py_tail_call_funcptr table[256];
    for (int i = 0; i < 256; i++) {
        table[i] = rq_abi_wrong;
    }
    enum { PROBE_OP = 0x5a, PROBE_ARG = 0xa5 };
    table[PROBE_OP] = rq_abi_probe;
    /* A frame-sized scratch area: the core's NOP writes frame->instr_ptr only. */
    union { _PyInterpreterFrame f; char pad[sizeof(_PyInterpreterFrame) + 64]; } fake;
    memset(&fake, 0, sizeof(fake));
    _Py_CODEUNIT code[2];
    code[0].op.code = NOP; code[0].op.arg = 0;
    code[1].op.code = PROBE_OP; code[1].op.arg = PROBE_ARG;
    _PyStackRef *sp = (_PyStackRef *)(uintptr_t)0x5151510;
    PyThreadState *ts = (PyThreadState *)(uintptr_t)0x7171710;
    memset(&rq_abi_seen, 0, sizeof(rq_abi_seen));
    PyObject *r = ((py_tail_call_funcptr)core_nop)(&fake.f, sp, ts, &code[0], table, 0);
    return r == (PyObject *)&rq_abi_seen && rq_abi_seen.calls == 1
           && rq_abi_seen.frame == &fake.f && rq_abi_seen.sp == sp && rq_abi_seen.tstate == ts
           && rq_abi_seen.next_instr == &code[1] && rq_abi_seen.table == (const void *)table
           && rq_abi_seen.oparg == PROBE_ARG && fake.f.instr_ptr == &code[0];
}

static int
rq_group_arm(int group, void **cells, void *const *orig, void *const *mine, int n)
{
    if (group < 0 || group >= RQ_NGROUPS || n > RQ_MAXOPS) {
        return -1;
    }
    rq_group_t *g = &rq_groups[group];
    RQ_ST(g->armed, 0);
    for (int i = 0; i < n; i++) {
        g->cell[i] = cells[i];
        g->orig[i] = orig[i];
        g->mine[i] = mine[i];
    }
    g->n = n;
    RQ_ST(g->total, 0ull);
    RQ_ST(g->hits, 0ull);
    RQ_ST(g->fills, 0ull);
    RQ_ST(g->window, 0u);
    RQ_ST(g->windows, 0);
    RQ_ST(g->suspended, 0);
    RQ_ST(g->armed, 1);
    return 0;
}

static void
rq_group_stats(int group, unsigned long long out[5])
{
    rq_group_t *g = &rq_groups[group];
    out[0] = RQ_LD(g->total) + RQ_LD(g->window);
    out[1] = RQ_LD(g->hits);
    out[2] = RQ_LD(g->fills);
    out[3] = (unsigned long long)RQ_LD(g->suspended);
    out[4] = (unsigned long long)RQ_LD(g->windows);
}

static const rq_api rq_api_desc = {
    .abi = RQ_API_ABI,
    .tag = RQ_RELEASE_TAG,
#ifdef Py_GIL_DISABLED
    .gil_disabled = 1,
#else
    .gil_disabled = 0,
#endif
    .init = rq_init,
    .handler = rq_handler,
    .group_of = rq_group_of,
    .abi_selftest = rq_abi_selftest,
    .group_arm = rq_group_arm,
    .group_stats = rq_group_stats,
    .opname = rq_opname,
    .getattr_fast = rq_getattr_fast,
};

#ifdef RQ_PGO_GEN
/* Instrumented build (tools/pgo.sh): pull in the profile runtime, which the Rust linker does
 * not request (clang's driver would pass -u__llvm_profile_runtime). */
extern int __llvm_profile_runtime;
__attribute__((used)) static int *rq_pgo_runtime = &__llvm_profile_runtime;
#endif

#define RQ_CAT2(a, b) a##b
#define RQ_CAT(a, b) RQ_CAT2(a, b)
__attribute__((visibility("hidden"))) const rq_api *
RQ_CAT(rq_api_, RQ_SET)(void)
{
    return &rq_api_desc;
}
