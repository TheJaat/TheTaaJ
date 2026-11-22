/* floats.c - the float versions of libm's functions (TheTaaJ's own; the
 * rest of this directory is musl's).
 *
 * Each is the double function, rounded once more to a float. The double
 * answer is within one ulp of a double of the true value, which is far
 * closer than a float can tell apart, so rounding it to a float gives
 * the nearest float, except when the true value lies almost exactly
 * halfway between two floats; then it can be the other of the two, one
 * ulp off. That is the accuracy musl's own float functions promise,
 * with a sixth of the code. They are slower; nothing on TheTaaJ needs
 * them fast yet. */

#include <math.h>

#define F1(name) float name##f(float x) { return (float)name((double)x); }
#define F2(name) float name##f(float x, float y) { return (float)name((double)x, (double)y); }

F1(sin) F1(cos) F1(tan) F1(asin) F1(acos) F1(atan) F2(atan2)
F1(sinh) F1(cosh) F1(tanh) F1(asinh) F1(acosh) F1(atanh)
F1(exp) F1(exp2) F1(expm1) F1(log) F1(log2) F1(log10) F1(log1p)
F2(pow) F1(cbrt)
