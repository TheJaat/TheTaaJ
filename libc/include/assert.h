/* assert.h - assert(condition): if the condition is false, says which
 * one, where, and ends the program (abort). With NDEBUG defined before
 * this is included, assert does nothing and the condition is not even
 * evaluated. Not guarded against being included twice, on purpose: C
 * lets a program include it again after changing NDEBUG. */
#undef assert

#ifdef NDEBUG
#define assert(condition) ((void)0)
#else
_Noreturn void __taaj_assert_failed(const char *condition, const char *file, int line, const char *function);
#define assert(condition) ((condition) ? (void)0 : __taaj_assert_failed(#condition, __FILE__, __LINE__, __func__))
#endif
