# `printf`: The First C in the C Library

*Journal entry #39 · a C program can print formatted text — `printf`, `fprintf`, `snprintf`, `puts` and their relations — written in C, and tested by making it agree, format by format, with the `printf` of the machine I write on*

For three entries every C test program has carried its own function for printing a number, with the same apology in the comment at the top: *there is no printf yet, so it writes its numbers itself.* Now:

```c
printf("printf: %d %d %x %#X [%5s] [%-5s] [%05d] %c 100%%\n", 42, -7, 255, 42, "hi", "hi", 42, 'x');
```

```
taaj:/> cprint one "two words"
printf: 42 -7 ff 0X2A [   hi] [hi   ] [00042] x 100%
sizes: 18446744073709551615 -9223372036854775808
12345
printf returned 6
snprintf: wanted 12, kept <hello, >
argument 1 of 2: one
argument 2 of 2: two words
puts: a line of its own
fputs and putchar
cprint: 9 lines went to standard output
```

This is step 4 of the seven towards a C library: (1) toolchain and crt0 — entry 36; (2) standard error — 37; (3) memory and strings — 38; (4) **`printf` — this entry**; (5) file descriptors; (6) `stdio` on files; (7) a real program and the recipe.

---

## Why this part is in C

The library so far is Rust with C names on it, and I said in entry 36 why: everything a C program needs done, the runtime already does. `printf` is the exception, and the reason is three dots:

```c
int printf(const char *format, ...);
```

A function with `...` takes *any number of arguments of any types*. Nothing in the call says what they are — only the format string does, at run time, one `%` at a time. The mechanism for walking such arguments is part of C itself:

```c
va_list arguments;
va_start(arguments, format);       /* begin after the last named parameter */
int n = va_arg(arguments, int);    /* "the next one is an int": take it */
char *s = va_arg(arguments, char *);
va_end(arguments);
```

`va_arg` is told the type and trusts it completely. Say `int` where a `long` was passed and it reads half of it; say `char *` where an `int` was passed and the program follows a small number as if it were an address. This is the most famous sharp edge in C, and it is exactly why Rust does not let me *define* such a function in ordinary stable Rust. C is the natural language for the one function built around it. So the plan from the start was: *Rust core, C on top* — and this is the first "on top".

## Two halves

```
                    format.c                          stdio.c
        ┌──────────────────────────────┐    ┌───────────────────────────┐
        │  __taaj_format(emit, ctx,    │    │  printf  fprintf  vprintf │
        │                format, args) │◄───│  vfprintf  puts  fputs    │
        │                              │    │  putchar  putc  fputc     │
        │  snprintf  vsnprintf         │    │  stdout  stderr           │
        │  sprintf   vsprintf          │    └─────────────┬─────────────┘
        └──────────────────────────────┘                  │ write(fd, …)
         asks the system for nothing                      ▼
         → can be tested on the host            the Rust half of libc.a
```

The split is the design. `format.c` turns a format and arguments into text, and **does not know where the text goes**. It is handed a function to call with each piece:

```c
typedef void (*emit_fn)(void *context, const char *bytes, size_t count);

int __taaj_format(emit_fn emit, void *context, const char *format, va_list arguments);
```

`snprintf` gives it a function that copies into a buffer and stops when the buffer is full. `printf` gives it one that gathers 256 bytes at a time and calls `write`. One formatter, two destinations — and because the formatter touches nothing but memory, it compiles and runs on Linux as well as here.

(`emit_fn` is a *function pointer*: a variable holding the address of a function. The `void *context` beside it is how C does what a Rust closure does — the function gets back whatever private data it needs, as an untyped pointer it casts to its own struct.)

## What a conversion is

```
%  [flags]  [width]  [.precision]  [length]  conversion
   - 0 + space #     5 or *        .3 or .*   hh h l ll z j t     d i u x X o c s p %
```

The loop in `__taaj_format` reads exactly in that order: ordinary text is passed through untouched up to the next `%`; then flags, then a width, then a precision, then a length, then one letter that says what to do.

| Piece | Means | Example → output |
|---|---|---|
| `-` | against the left edge | `[%-5d]` 42 → `[42   ]` |
| `0` | pad with zeros, after the sign | `[%05d]` -42 → `[-0042]` |
| `+` / space | always a sign / a space for it | `%+d` 5 → `+5` |
| `#` | `0x` before hex, `0` before octal | `%#x` 255 → `0xff` |
| width | the *least* room to take | `[%5s]` hi → `[   hi]` |
| precision, number | the *least* digits | `%.3d` 5 → `005` |
| precision, string | the *most* bytes | `%.3s` abcdef → `abc` |
| `*` | take that number from the arguments | `%*d` 6, 42 → `    42` |
| length | how big the argument is | `%zu` for a `size_t`, `%lld` for a `long long` |

### Why the length matters, and one thing I had not known

`%d` and `%ld` print the same digits. The length is not about printing; it is about **how many bytes `va_arg` must take off the argument list**. Get it wrong and this argument is wrong and so is every one after it.

And the small types have a twist. When a `char` or a `short` is passed through `...`, C quietly widens it to an `int` first (the "default argument promotions"). So `%hhd` cannot fetch a `char` — there is no `char` there to fetch:

```c
case CHAR:  value = (signed char)va_arg(arguments, int); break;
case SHORT: value = (short)va_arg(arguments, int); break;
```

It fetches an `int` and cuts it back down. `printf("%hhd", 200)` prints `-56`: 200 does not fit a signed byte.

### One function for every whole number

All the integer conversions end in the same function, which writes these pieces in this order:

```
[spaces] [sign] [0x] [zeros] digits [spaces]
```

```c
static void put_number(struct output *out, const struct spec *spec, uintmax_t value, int negative,
                       int is_signed, unsigned base, int capitals)
```

The value arrives **unsigned, with the sign as a separate flag**. That is for one number: the most negative one. `-9223372036854775808` has no positive twin — `-value` overflows, which in C is not "wraps round" but *undefined*. Computing the size in unsigned arithmetic, where wrapping is defined, makes the special case disappear:

```c
uintmax_t size = value < 0 ? 0 - (uintmax_t)value : (uintmax_t)value;
```

The digits are produced last-first into the end of a small buffer (dividing by the base gives the *last* digit first), and then the rules about zeros and padding are just arithmetic on four lengths.

## `snprintf`, and why its return value is odd

```c
char small[8];
int wanted = snprintf(small, sizeof small, "%s, %s", "hello", "world");
/* small is "hello, " and wanted is 12 */
```

`snprintf` never writes more than the size it is told, always ends the text with a zero — and returns **how long the whole text would have been**, not how much it wrote. That looks strange until the use is seen: `wanted >= sizeof small` is how a caller learns the text was cut, and `snprintf(NULL, 0, …)` is how to ask "how big a buffer do I need?" before `malloc`ing one.

`sprintf` — no `n`, no size — is here too because old programs call it. It writes as far as the text goes, into a buffer it takes on trust.

## `printf`, and what a stream is for now

```c
struct _FILE {
    int descriptor;
};

FILE *stdout = &standard_output;   /* descriptor 1 */
FILE *stderr = &standard_error;    /* descriptor 2 */
```

A `FILE` is, today, a descriptor with a name. Nothing is kept between calls. *Within* one call, the pieces the formatter emits are gathered in a 256-byte buffer on the stack and written when it fills and once at the end — so `printf("%d: %s\n", n, name)` is one `write`, not five, and a line printed by one program cannot be cut in the middle by another's.

When `printf` returns, everything it printed has been written. That is simpler than real `stdio` (which keeps text until a line or a buffer is full) and it means `printf` to standard output and `fprintf(stderr, …)` always come out in the order they were called. Buffered streams and files are step 6.

## The compiler reads the format too

One line in the header buys a great deal:

```c
int printf(const char *format, ...) __attribute__((format(printf, 1, 2)));
```

It tells the compiler "argument 1 is a printf format; check the arguments from 2 on against it". The sharp edge of `va_arg` is covered at compile time:

```
bad.c:2:33: error: format specifies type 'char *' but the argument has type 'int' [-Werror,-Wformat]
    2 | int main(void) { printf("%s\n", 5); return 0; }
```

## Testing it against forty years of other people's bug reports

How do I know `[%-#10x|` or `%.0d` of zero comes out right? I could read the C standard for each case and write down the expected string. I would make the same mistakes in the expectation as in the code.

But there is a `printf` on my desk that has had decades of people finding its mistakes: the host's. And `format.c` runs on the host. So the test compiles `format.c` for Linux with its functions renamed —

```make
FORMAT_RENAMED = -Dvsnprintf=taaj_vsnprintf -Dsnprintf=taaj_snprintf …
```

— and a macro gives the same format and arguments to both:

```c
#define SAME_IN(size, ...)                                              \
    do {                                                                \
        char theirs[512], ours[512];                                    \
        memset(theirs, '#', sizeof theirs);                             \
        memset(ours, '#', sizeof ours);                                 \
        int their_count = snprintf(theirs, (size), __VA_ARGS__);        \
        int our_count = taaj_snprintf(ours, (size), __VA_ARGS__);       \
        if (their_count != our_count || memcmp(theirs, ours, sizeof theirs) != 0) {  … report … }
    } while (0)

SAME("[%08.3d]", 42);
SAME("%jd %ju", INTMAX_MIN, UINTMAX_MAX);
SAME_IN(8, "hello, world");
```

Both buffers are filled with `#` first and compared **whole**, all 512 bytes — so a byte written past the zero, where no string comparison would look, is a failure too.

The renaming is the same trick as `cfg_attr(not(test), no_mangle)` in the last entry, from the other side: on the host, my `snprintf` must not *be* `snprintf`, or I would be comparing it with itself.

123 checks. Four of them are not comparisons, because there the host and I legitimately differ and I chose: a null string prints `(null)`; a null address prints `0x0` (glibc prints `(nil)`); a conversion I do not know is printed as it was written; and so is a format that ends in the middle of one.

The test is also built with the compiler's **sanitizers** (`-fsanitize=address,undefined`): extra checks compiled into the program that stop it the moment it reads or writes outside a buffer, or does arithmetic C leaves undefined. That found the entry's one real bug.

## The design, in one table

| Decision | What I chose | What I rejected, and why |
|---|---|---|
| Language | C, for this part only | Rust — defining a function that takes `...` is not something stable Rust lets me do; and the plan was always C on top |
| How the library is built | cargo makes the Rust archive; Clang compiles `libc/c/*.c`; `ar` adds those objects to a *copy*, `build/c/libc.a` | Linking the C objects beside the archive on every program's command line — then "the C library" is not one file, and the recipe for one's own program gets longer |
| Shape | A formatter that calls an `emit` function; `snprintf` and `printf` on top | `printf` formatting into a big buffer with `vsnprintf` and writing that — simple, but then a long `%s` needs a buffer as long as the string, or is cut |
| Buffering | 256 bytes within one call; nothing kept between calls | Line buffering now — it is the right end state, and belongs with `FILE` becoming real in step 6 |
| Floating point | Not there: `%f %e %g %a` print as written | A simple `%f` — printing a `double` *correctly* (rounding, the shortest digits that read back) is a project of its own, and a wrong `%f` is worse than a visible gap |
| `%n` | Not there | It *writes* through a pointer taken from the arguments: the heart of most format-string attacks. Many libraries disable it |
| Unknown conversion | Printed as written | Printing nothing, or stopping — a mistake in a format should show on the screen |
| A null `%s` | `(null)` | Crashing, which the standard allows — unhelpful |
| Host test | Compare with the host's `snprintf`, sanitizers on | Hand-written expected strings — my misreadings would be in both the code and the test |
| In-system test | One program whose output is compared byte for byte from a file | Repeating the format checks in the OS — slow, and they test nothing the host cannot |

## The bugs

**A null pointer, moved by nothing.** `snprintf(NULL, 0, "%d apples", 12)` is legal and useful — it asks only for the length. My function for copying into the buffer worked out that zero bytes fit, and then did this:

```c
memcpy(buffer->to + buffer->used, bytes, fits);   /* to == NULL, used == 0, fits == 0 */
```

Copying zero bytes harms nothing, and on every machine I know the result is right. But `NULL + 0` is pointer arithmetic on a null pointer, which C does not define, and the sanitizer stopped the test:

```
libc/c/format.c:345:23: runtime error: applying zero offset to null pointer
```

Every one of the 123 comparisons would have *passed* without the sanitizer; the output was identical to the host's. This is the kind of bug that works for years and then stops working when a compiler gets cleverer about what "undefined" permits. The fix is to return before touching the pointer when nothing fits.

**`1 lines`.** My expectation for `cprint long | wc` said `1 lines, 1 words, 601 bytes`. `wc`, which I wrote, says `1 line, 1 word`. The program was right and the test was wrong — and I found I had written the same wrong phrase in the *last* entry's testing note, in a row marked "not in the automated test". Corrected there too. Untested lines in a testing note are where this hides.

## How I checked it

**Automated** (`make test64`):

- **Host** (`make libc-test`, part of the host tests): 123 checks of the formatter, 119 of them against the host's `snprintf`, built with the address and undefined-behaviour sanitizers. Numbers at every edge (`INT_MIN`, `LLONG_MIN`, `UINTMAX_MAX`), every length, every flag alone and in the combinations that interact, width and precision from `*` including negative ones, strings cut by precision including one with no zero at its end, addresses, long padding, and buffers too small (sizes 8, 6, 5, 4, 3, 1, 0, and no buffer at all).
- **In the booted system, BIOS and UEFI:**
  - `cprint one "two words"` — nine lines in the log, each matched, and `cprint: 9 lines went to standard output`.
  - `cprint long | wc` — `1 line, 1 word, 601 bytes`: one `printf` of more than twice its buffer arrives whole.
  - `cprint > pout` — the host reads `pout` from the disk and compares it **byte for byte** with the seven lines expected; the `fprintf(stderr, …)` line must be in the log and not in the file.
- `ls` must count 20 files in the root.

**By hand:** the two compiler checks quoted above (`printf("%s", 5)` is refused; a format with `%f` compiles).

**Not tested:**

- `vprintf`, `vfprintf`, `vsprintf` called directly by a program (they are what the tested functions are made of).
- `printf` when the write fails — the `EOF` return.
- Two programs printing at once, to see that lines do not mix.
- Any program printing megabytes through `printf`.

## What is still missing

- **No floating point in `printf`.** `%f` and its relations compile (the compiler cannot know) and print as `%f`; worse, the `double` is then not taken off the argument list, so any later conversion *in the same call* gets the wrong argument.
- No `%n`, by choice. No wide characters (`%lc`, `%ls`).
- `FILE` is a descriptor and nothing more: no buffering between calls, no `fflush`, no `fopen`, no `stdin`, no `scanf`, no `getchar`.
- No `perror`, no `errno`.
- The library is built in two steps and exists only in `build/c/` — nothing installs it anywhere.
- `chello` and `cmem` still print their numbers by hand. They are the record of what C here could do when they were written; I left them.
- `argv[0]` is still the placeholder `program`.

## What I learned

- **`...` is a promise the caller makes and nobody checks** — except that one attribute line in a header gets the compiler to check it for every call with a literal format. I had used `printf` for years without knowing the warnings came from an annotation anyone can write.
- **The length in `%ld` is about fetching, not printing.** And `char` and `short` do not survive the trip through `...` at all.
- **Separate what is computed from where it goes.** The `emit` function is three lines of design and it bought both `snprintf` for free and a formatter that can be tested on another operating system.
- **The best test oracle is an old, heavily used implementation of the same thing.** Differential testing: I did not have to know the right answer to 119 questions, only notice a disagreement.
- **Identical output is not the same as correct.** The sanitizer found a bug that no comparison of results could, because the result was right. "Undefined behaviour" is a property of the program, not of what it printed today.
- **A function pointer plus a `void *` is a closure by hand.** Having met closures in Rust first, the C idiom finally made sense backwards.
- **Untested claims rot first.** The wrong `1 lines` sat in a row I had marked as not automated.

---

## Testing note

```
make run64
```

**`printf`**

| Type | You should see |
|---|---|
| `cprint` | `printf: 42 -7 ff 0X2A [   hi] [hi   ] [00042] x 100%` |
| | `sizes: 18446744073709551615 -9223372036854775808` |
| | `12345` |
| | `printf returned 6` |
| | `snprintf: wanted 12, kept <hello, >` |
| | `puts: a line of its own` |
| | `fputs and putchar` |
| | `cprint: 7 lines went to standard output` |
| `cprint one "two words"` | The same, with `argument 1 of 2: one` and `argument 2 of 2: two words` after the `snprintf` line, and `9 lines` at the end |

**Whole, wherever it goes**

| Type | You should see |
|---|---|
| `cprint long` | 600 `x`s (they wrap over several rows of the screen) |
| `cprint long \| wc` | `1 line, 1 word, 601 bytes` |
| `cprint > pout` | Only `cprint: 7 lines went to standard output` — that line is on standard error |
| `cat pout` | The seven lines, without the `cprint:` one *(the `cat` is not automated; the host reads the file)* |
| `cprint \| wc` | The `cprint:` line, then `7 lines, 34 words, 204 bytes` *(not in the automated test)* |

**On the host**

```
make libc-test
```

ends with `format: 123 checks, all the same as the host's printf` (after the 10 string tests).

**A program of your own** *(not tested: please try it)* — on the host:

```
cat > libc/programs/mine.c <<'EOF'
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv)
{
    for (int n = 1; n < argc; n++)
        printf("%2d. %-10s|%3zu letters\n", n, argv[n], strlen(argv[n]));
    return 0;
}
EOF
make cprogs64 C_PROGRAMS=mine
tools/tfstool-rs/target/release/tfstool put build/taaj-disk.img mine build/c/mine
```

(with QEMU closed), then `make run64` and type `mine apple fig`: you should see

```
 1. apple     |  5 letters
 2. fig       |  3 letters
```

And one that must **not** compile — change `strlen(argv[n])` to `argv[n]` in the `printf` line and run `make cprogs64 C_PROGRAMS=mine` again: the compiler should refuse, saying the format specifies `size_t` but the argument is `char *`. Delete `libc/programs/mine.c` afterwards, and `rm mine` in the OS.

**From the host** — with QEMU closed:

```
tools/tfstool-rs/target/release/tfstool get build/taaj-disk.img pout
```

prints the seven lines.

**Cleaning up**, in the OS: `rm pout`.

Automated: `make test64`.
