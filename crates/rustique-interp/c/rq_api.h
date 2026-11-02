/* The interface of one compiled handler set, shared with src/lib.rs (#[repr(C)] mirrors).
 * Bump RQ_API_ABI on any change. */
#ifndef RQ_API_H
#define RQ_API_H

#define RQ_API_ABI 1

typedef struct {
    void *core_handler_table;    /* &instruction_funcptr_handler_table[0] of the core */
    void *core_tracing_table;    /* &instruction_funcptr_tracing_table[0], NULL without tier 2 */
    void *type_lookup_ref;       /* _PyType_LookupStackRefAndVersion (needed on t builds) */
    void (*on_suspend)(int group);
} rq_init_args;

typedef struct {
    unsigned int abi;
    const char *tag;             /* the CPython release the set was generated from */
    int gil_disabled;
    int (*init)(const rq_init_args *);          /* once per process; 0 or -1 */
    void *(*handler)(int opcode);               /* our handler, or NULL if not swappable */
    int (*group_of)(int opcode);                /* override group of a swappable opcode */
    int (*abi_selftest)(void *core_nop);        /* 1 if the calling convention matches */
    int (*group_arm)(int group, void **cells, void *const *orig, void *const *mine, int n);
    void (*group_stats)(int group, unsigned long long out[5]);
    const char *(*opname)(int opcode);          /* name of a swappable opcode, or NULL */
} rq_api;

#endif
