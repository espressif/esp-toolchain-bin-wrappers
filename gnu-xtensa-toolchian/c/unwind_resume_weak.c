/*
 * MinGW for 32-bit Windows often lacks DWARF unwinder / _Unwind_Resume.
 * Rust still references it with panic=abort (rust#79609).
 * Weak stub: real symbol from another TU wins if present; else linker satisfied.
 */
void __attribute__((weak)) _Unwind_Resume(void *exc)
{
    (void)exc;
}
