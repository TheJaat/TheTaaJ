# A Real C Program, and the Recipe

*Journal entry #42 · a word counter with a hash table, written in standard C and nothing else — the same file compiled for TheTaaJ and for Linux, and their outputs compared byte for byte; the three pieces of library it turned out to need (`ctype.h`, `strtol`, `qsort`); and a page that says how to build a C program of your own*

```
taaj:/> echo the cat and the hat and the bat > t
taaj:/> with readonly cwords -n 2 t
      3  the
      2  and
8 words, 5 different
```

Six entries built a C library. Each one had a test program — `chello`, `cmem`, `cprint`, `cfile`, `cstdio` — and each of those was written *to exercise the library*: it calls the functions the entry added, in the ways I thought of. That proves the functions do what I meant. It does not prove the library is enough to write a program with.

So this entry does the opposite. I designed a program first — an ordinary one, the way I would write it for Linux, using whatever standard C it wanted — and then looked at what the library did not have.

This is step 7 of seven towards a C library, the last: (1) toolchain and crt0 — 36; (2) standard error — 37; (3) memory and strings — 38; (4) `printf` — 39; (5) file descriptors — 40; (6) `stdio` on files — 41; (7) **a real program and the recipe — this entry**.

---

## The program

`cwords` reads text and prints the words used most often:

```
cwords [-n COUNT] [FILE...]
```

A word is a run of letters and digits; capitals and small letters are the same word. It reads the files named, or standard input if none.

I chose it because it is small enough to read in one sitting (about 200 lines) and still needs most of what a real program needs: an option with a number, files *or* standard input, memory that grows with the input, a data structure, sorting, and output that must be exactly right.

### Why a hash table

The question the program asks, for every word in the text, is: *have I seen this word before, and if so, where is its count?*

The simple answer is a list of all words seen so far, searched from the start each time. For a text with 10 000 different words that is thousands of comparisons per word.

A **hash table** makes it about one. The idea:

1. Turn the word into a number — any number, as long as the same word always gives the same one and different words usually give different ones. This is the *hash*.
2. Keep an array of *slots*. The word's slot is `hash % number_of_slots`.
3. Each slot holds a short chain of the words that landed there. Look only along that chain.

```
slots:   [0]   [1]        [2]   [3]             ...
          |     |          |     |
         NULL  "the":3    NULL  "cat":1 -> "hat":1
```

```c
struct entry {
    char *word;
    long count;
    struct entry *next; /* the next word that landed in the same slot */
};

static void count_word(const char *word)
{
    total++;
    if (different >= slot_count)
        grow();
    size_t slot = hash(word) % slot_count;
    for (struct entry *entry = slots[slot]; entry != NULL; entry = entry->next) {
        if (strcmp(entry->word, word) == 0) {
            entry->count++;
            return;
        }
    }
    /* ... not found: make an entry, and put it at the head of the chain ... */
}
```

Two details make it work in practice.

**The hash must spread.** If "cat", "hat" and "bat" all gave nearby numbers, they would crowd into nearby slots. I used FNV-1a, which is two lines — for each byte, XOR it in, then multiply by a large prime:

```c
static unsigned long hash(const char *word)
{
    unsigned long number = 14695981039346656037ul;
    for (; *word != '\0'; word++) {
        number ^= (unsigned char)*word;
        number *= 1099511628211ul;
    }
    return number;
}
```

The multiplication overflows on purpose. For an *unsigned* type C defines that: the result wraps round. (For a signed type it would be undefined behaviour — the reason the type is `unsigned long`.)

**The table must grow.** With 64 slots and 10 000 words, every chain would be 150 long and the table would be a slow list again. So whenever there are as many words as slots, `grow` makes a table twice the size and moves every entry to its new slot. The chains stay about one entry long however much text arrives.

### Sorting, and a tie-break that matters

At the end the entries are gathered into an array and sorted with `qsort`: most used first.

```c
static int by_count(const void *one, const void *other)
{
    const struct entry *a = *(const struct entry *const *)one;
    const struct entry *b = *(const struct entry *const *)other;
    if (a->count != b->count)
        return a->count > b->count ? -1 : 1;
    return strcmp(a->word, b->word);
}
```

The last line is not decoration. Without it, two words with the same count would come out in whatever order the hash table and the sort happened to leave them — and that order would differ between two libraries with different `qsort`s. With it, the output is fully decided by the input. That is what lets me compare this program's output across two systems at all.

(The cast is the price of `qsort` not knowing what it sorts: it hands the comparison function two `const void *` — addresses of two array items. The items here are themselves pointers, so each argument is a pointer *to a pointer to an entry*.)

## What was missing

I listed what `cwords` would call and held the list against the headers in `libc/include/`. Three things were not there:

- `ctype.h` — no header at all (`isalnum`, `tolower`);
- `strtol` — for `-n COUNT`;
- `qsort` — for the ordering at the end.

Everything else — `fopen`, `getc`, `malloc`, `realloc`, `calloc`, `free`, `printf` with `%7ld` and `%zu`, `strcmp`, `strerror`, `errno`, `exit` — was already there. Three gaps in two hundred lines is, I think, an honest measure of where the library stands. I filled them, plus their obvious neighbours.

### `ctype.h`: no table needed

`isalpha`, `isdigit`, `isalnum`, `isupper`, `islower`, `isxdigit`, `isspace`, `isblank`, `isprint`, `isgraph`, `ispunct`, `iscntrl`, `toupper`, `tolower`.

Many C libraries answer these from a table of 257 entries. ASCII was designed so that they are comparisons:

```c
int isdigit(int c) { return c >= '0' && c <= '9'; }
int isspace(int c) { return c == ' ' || (c >= '\t' && c <= '\r'); }
int isprint(int c) { return c >= ' ' && c <= '~'; }
int toupper(int c) { return islower(c) ? c - ('a' - 'A') : c; }
```

The digits are in a row; each alphabet is in a row; a capital and its small letter differ by exactly 32, one bit; everything printable lies between the space and the tilde; and the five "white space" control characters (tab, new line, vertical tab, form feed, carriage return) are numbers 9 to 13.

These take an `int`, not a `char`, for the reason `getc` returns one (entry 41): the argument may be any byte 0–255 *or* `EOF`. So `isalnum(getc(file))` is correct as written, but `isalpha(text[n])` is a bug wherever `char` is signed and the byte is 128 or more — it passes a negative number. The cast `(unsigned char)text[n]` is the fix, and my own `strtol` has it in four places.

Bytes from 128 up are *nothing* here: not letters, not printable. Text on this system is UTF-8, where those bytes are pieces of a longer character, and one piece alone is not a letter.

### `strtol`: the function that can say "that was not a number"

`atoi("12")` is 12. `atoi("none")` is 0. `atoi("0")` is 0. That is why `atoi` is not enough: it cannot tell a zero from a failure. `strtol` can, through its second argument:

```c
char *end;
errno = 0;
most = strtol(argv[2], &end, 10);
if (end == argv[2] || *end != '\0' || errno == ERANGE || most < 1) {
    fprintf(stderr, "cwords: -n needs a number from 1 up, not \"%s\"\n", argv[2]);
    return 2;
}
```

`end` is left pointing just after the last digit used. So:

- `end == argv[2]` — it did not move: there was no number at all;
- `*end != '\0'` — it stopped before the end: `12abc`;
- `errno == ERANGE` — the digits were a number too big for a `long`.

Three different failures, and `atoi` reports none of them.

Inside, the interesting part is overflow. The value is built up digit by digit as `value * base + digit`, and the question "would that overflow?" has to be asked **before** doing it, because afterwards the evidence is gone:

```c
if (*value > (ULONG_MAX - (unsigned long)digit) / (unsigned long)base)
    too_big = 1;
else
    *value = *value * (unsigned long)base + (unsigned long)digit;
```

This is the inequality `value * base + digit > ULONG_MAX` rearranged so that nothing in it can itself overflow.

`strtol` and `strtoul` share that loop, which builds the digits' value as an unsigned number without the sign. Then each applies its own limit. For `strtol` the limit depends on the sign, because a `long` reaches one further below zero than above:

```c
unsigned long most = negative ? (unsigned long)LONG_MAX + 1 : (unsigned long)LONG_MAX;
```

Two rules of C's that I would have got wrong without the host to compare against:

- **`"0x"` is the number 0 followed by an `x`.** The prefix counts as a prefix only if a hex digit follows it.
- **`strtoul("-1")` is not an error.** It is 18446744073709551615: C says the minus sign negates the result *in the unsigned type*.

### `qsort`: not quicksort

Despite its name, C does not say `qsort` must be quicksort, and mine is not. It is **heap sort**.

Quicksort is the faster of the two on a good day. On a bad day — input in an unlucky order, which for a simple quicksort is *already sorted* input — it takes time that grows with the square of the number of items, and calls itself as deep as there are items. A program's stack here is small — 16 pages, 64 KiB. I did not want a library function whose worst case is a crash.

Heap sort takes the same time whatever it is given, uses no extra memory, and calls nothing.

The idea is to see the array as a tree: the children of item *n* are items *2n+1* and *2n+2*.

```
array:   [ 9 | 7 | 8 | 3 | 5 | 6 ]

tree:           9
              /   \
             7     8
            / \   /
           3   5 6
```

A *heap* is such a tree where no child is greater than its parent — so the greatest item is at the top. Then:

1. Make the array into a heap.
2. Swap the top (the greatest) with the last item. It is now in its final place.
3. The heap is one shorter, and its new top is probably wrong: let it *sink* — swap it with its greater child until neither child is greater.
4. Repeat from 2.

```c
void qsort(void *base, size_t count, size_t size, int (*compare)(const void *, const void *))
{
    unsigned char *items = base;
    if (count < 2 || size == 0)
        return;
    for (size_t top = count / 2; top-- > 0;)
        sink(items, top, count, size, compare);
    for (size_t last = count - 1; last > 0; last--) {
        exchange(items, items + last * size, size);
        sink(items, 0, last, size, compare);
    }
}
```

`qsort` knows nothing about what it sorts. An item is "`size` bytes at this address"; item *n* is at `items + n * size`; swapping two items is swapping bytes. That is the whole meaning of `void *` in C: an address with the type taken off, to be put back on by whoever knows what it is — here, the comparison function.

`for (size_t top = count / 2; top-- > 0;)` is the idiom for counting *down to zero* with an unsigned number. `top >= 0` would always be true — an unsigned number cannot be negative — so the test and the decrement are done together: "is it above zero? then step down, and use that".

`bsearch` came along with it: binary search over a sorted array, a dozen lines.

## The test: the same file, on two systems

`cwords.c` uses standard C and nothing else. So the Makefile compiles it twice:

```
clang --target=x86_64-unknown-none-elf ... cwords.c  +  build/c/libc.a   ->  build/c/cwords       (TheTaaJ)
cc -std=c11 -Wall -Wextra -Werror          cwords.c  +  the host's libc  ->  build/c/host-cwords  (Linux)
```

In the booted system the test types:

```
with readonly cwords -n 25 cwords long > cw1
cwords -n 5 < long > cw2
```

and afterwards, on the host, the boot checker runs the host's copy on the same inputs and compares:

```sh
"$CWORDS_HOST" -n 25 "$CWORDS_TAAJ" "$DISK.long" > "$LOG.want"
get cw1 | cmp -s - "$LOG.want" || fail "..."
```

The first input is deliberately nasty: `cwords` reading **its own machine code** — 54 KB of binary, every byte value, with runs of letters and digits scattered through it. That alone is 9823 "words", 562 of them different; with the 300-line text file after it, 11 023 and 850: enough to grow the hash table several times, to push bytes above 127 through `isalnum`, and to make many ties for the sort to break.

For the two outputs to be identical, all of this has to agree with glibc: `getc` and the buffer under it, `isalnum`, `tolower`, `realloc` keeping the bytes, the hash arithmetic, `qsort` with a comparison function, `strcmp`'s ordering of bytes, and `printf("%7ld  %s\n")`. One wrong byte anywhere and `cmp` fails.

It is the strongest test in the whole C library, and I wrote none of its expected output.

The pieces also got their own host tests, in the style of entries 39 and 41 — compiled for the host, renamed with `objcopy`, compared with the host's:

- **ctype:** all 14 functions, for every argument from -1 to 255.
- **`strtol`/`strtoul`:** 55 awkward texts × 9 bases, comparing the value, where `end` was left, and `errno`.
- **`qsort`:** 3000 random arrays of ints (every seventh already sorted), compared with the host's result; strings; 20-byte records that must arrive with their cargo intact; single bytes; nothing at all.
- **`bsearch`:** every key present and absent, for 0 to 40 items.

7899 checks. Then I broke `stdlib.c` on purpose eight ways (wrong limit for negative numbers, `0x` without the digit rule, `end` not reset, the overflow test wrong, the sink choosing the wrong child, the heap built from the wrong half, binary search stepping too far, the byte swap one short) and the test noticed all eight.

## The recipe

The other half of this step is a page: **`docs/c-programs.md`** — how to write a C program for TheTaaJ. It has:

- the short way, now one command: **`make cput C=mine`** builds `libc/programs/mine.c` and puts it on the disk `make run64` uses;
- the long way — the three commands (`clang`, `ld.lld`, `tfstool put`) for a program of several files, or one kept outside the repository;
- what each compiler flag is for;
- what a program is given (arguments, the three streams, and files only when started `with files` or `with readonly`);
- every function the library has, by header;
- what it does not have, and what it has that behaves differently from Unix;
- a table of "what you see → what it means" for when it does not work.

I tested the page by following it: a throwaway `crecipe.c` through `make cput`, and a two-file program (`main.c` + `shout.c` with its own header) through the long way, both run in the booted system.

## The design, in one table

| Decision | What I chose | What I rejected, and why |
|---|---|---|
| What "a real program" is | One I wrote, in standard C only | Porting someone else's — the honest test, but anything worth porting needs the keyboard, `scanf`, floats in `printf` or `stat`, none of which exist yet |
| How to test it | The same source on the host; compare outputs | Hand-written expected output — tests only the cases I think of, and my arithmetic |
| Test input | The program's own binary, plus text | Text only — never exercises bytes ≥ 128, long "words", or a table that grows |
| Ties in the output | Broken alphabetically | Left to the sort — then two correct `qsort`s give two different outputs |
| `ctype` | Comparisons | A 257-entry table — faster by a hair, and 257 chances for a typo |
| Bytes ≥ 128 | Never letters | Latin-1 — wrong for UTF-8, which is what text here is |
| `qsort` | Heap sort | Quicksort — n² time and deep recursion on unlucky input, with a small stack |
| `strtol` and `strtoul` | One shared digit loop, two limits | Two copies — two places for the same overflow bug |
| New code in C or Rust | C (`libc/c/ctype.c`, `stdlib.c`) | Rust — these need nothing from the runtime, and in C they are tested against the host's with the same harness as `stdio.c` |
| Installing a program | `make cput C=name` | Only the documented three commands — fine once, tedious on the tenth rebuild |

## The bugs

**None in the library this time, and I do not fully trust that.** `ctype.c`, `stdlib.c` and `cwords.c` all passed their tests on the first run: 7899 host checks and both byte-for-byte comparisons. After entry 41 I know that "all passed" can mean "the test cannot fail", so the deliberate breaks above are what I am relying on, not the green line.

**`strdup` is not standard C.** My first `cwords.c` used it. That is fine on TheTaaJ (my `string.h` has it), but the program was to be standard C only, and `strdup` is POSIX: with `-std=c11`, glibc's `string.h` does not declare it, and the host build stops with `implicit declaration of function 'strdup'` (I checked with a two-line file). I noticed on re-reading, before the host build did it for me; `cwords` now does `malloc` + `strcpy`. My own header is more generous than the standard, and only the thought of compiling somewhere else showed it.

**A string that was two strings.** In the host test's list of awkward texts I wrote `"12\0" "34"` — a text with a zero byte in the middle — and the compiler refused it: two string literals side by side in an array look like a missing comma. It was right to ask. Written as `"12\00034"` (an octal escape is at most three digits, so `\000` is the zero and `34` follows).

**A wrong test key.** Checking the recipe by hand I typed `ls | cwords -n 1` through my key-sending helper, which dropped the `|`; the shell ran `ls cwords -n 1`. A mistake in a throwaway script, not in anything committed — but it is why the testing note marks that line as not tried.

## How I checked it

**Automated** (`make test64`, which ended `test64: all passed`):

- **Host, `make libc-test`:** `stdlib: 7899 checks, all the same as the host's` (described above), alongside the earlier `format: 123` and `stdio: 124`.
- **In the booted system, BIOS and UEFI:**
  - `cat long | cwords -n 3` → `301  300`, `300  line`, `300  of`, then `1200 words, 302 different`. (The word `300` appears in all 300 lines and once more as line number 300.)
  - `with readonly cwords -n 25 cwords long > cw1` → compared on the host with the host-built `cwords` on the same two files.
  - `cwords -n 5 < long > cw2` → the same, for standard input.
  - `cwords long` (no grant) → `cwords: long: this program was not given the files`, and still the summary `0 words, 0 different`.
  - `cwords -n none` → `cwords: -n needs a number from 1 up, not "none"`.
  - `cwords -x` → the usage line.

**By hand, once:**

- The eight deliberate breaks of `stdlib.c`.
- `make cput C=crecipe` with a five-line program, then booted: `made by the recipe`.
- The long way from `docs/c-programs.md` with a two-file program, then booted: `shout hello from two files` → `HELLO FROM TWO FILES!`.
- `with readonly cwords -n 2 t` on a file made with `echo`, as at the top of this entry.

**Not tested:**

- `cwords` running out of memory (the message and exit code 2).
- A text bigger than about 60 KB, or a word longer than a few dozen bytes on the target (the growing word buffer is exercised by the binary's long runs, but I have not checked how long they are).
- `-n` with a number too big for a `long` in the booted system (host test only).
- `ls | cwords` by hand; the automated test covers a pipe with `cat long | cwords`.
- `abs`, `labs`, `atoi`, `atol`, `strtoul` and `bsearch` in the booted system: `cwords` does not use them. They are the same C compiled by the same compiler, and host-tested.
- The recipe on a machine other than mine, or with a Clang older than the one I have.

## What is still missing

The library is now enough to write a text-processing tool. It is not enough to port most existing software. In the order I expect to hit them:

- **The keyboard as standard input.** Nothing interactive can be written in C. This is a shell feature as much as a library one: while a program runs, the shell would have to pass typed lines to it.
- **`scanf`** and its family.
- **Floating point in `printf`**, and `math.h`.
- **Directories and file facts:** `stat`, `mkdir`, `opendir`, `getcwd`.
- `getenv`, `time`, `atexit`, `abort`, `assert.h`, `setjmp.h`.
- Processes: `fork`, `exec`, `system`, `wait`, `pipe`, signals. Not planned in that form at all: this system starts programs differently.
- `argv[0]` is still the placeholder `program`.
- A C++ or Rust-std program is a different project (a different runtime under each).

## What I learned

- **A library is tested by a program that was not written to test it.** Six test programs found no gaps; one ordinary program needed three things that were not there.
- **"The same source on two systems, compare the output" is the cheapest strong test I have.** It needs no expected values, and it checks everything between `getc` and `printf` at once.
- **To compare across systems, output must depend only on input.** Hence the tie-break: without it both systems would be correct and still differ.
- **A hash table is an array, a function from keys to numbers, and a plan for collisions.** Growing it keeps the chains short; that is the whole trick.
- **Unsigned overflow wraps; signed overflow is undefined.** The hash relies on the first. `strtol` is written to never do the second.
- **Check for overflow before, not after.** Rearrange the inequality so that the check itself cannot overflow.
- **`strtol` exists because `atoi` cannot say no.** The `end` pointer is how a function with one return value reports a second thing.
- **`qsort` need not be quicksort.** The name is history. Worst case and stack depth matter more to a library than the average.
- **`void *` is an address with its type taken off.** Whoever takes it off is trusting someone else to put the right one back.
- **`for (n = count; n-- > 0;)`** counts an unsigned number down through zero.
- **ASCII's layout is a design**, not an accident: ranges for digits and letters, one bit between cases.
- **My header lets through something the standard does not have** (`strdup`). Another library's headers would refuse it. Portability is tested by porting.
- **Writing the instructions and then following them** found nothing wrong this time, and I still would not have trusted the page otherwise.

---

## Testing note

```
make run64
```

The disk `make run64` uses starts with only the programs on it, so the first lines make a text to count.

**Counting words**

| Type | You should see |
|---|---|
| `echo the cat and the hat and the bat > t` | Nothing |
| `cwords t` | `cwords: t: this program was not given the files`, then `0 words, 0 different`, then `cwords exited with code 1` |
| `with readonly cwords t` | `      3  the`, `      2  and`, then `bat`, `cat`, `hat` with `1` each, in that order, then `8 words, 5 different` *(checked by hand with `-n 2`; not in the automated test in this form)* |
| `with readonly cwords -n 2 t` | `      3  the`, `      2  and`, `8 words, 5 different` |
| `cwords -n 1 < t` | `      3  the`, then `8 words, 5 different` — standard input, no files asked for |
| `cat t \| cwords -n 1` | The same *(the automated test does this with a 300-line file)* |
| `echo The THE the > u` then `cwords < u` | `      3  the`, then `3 words, 1 different`: capitals are the same word. *(Not tested: the key typer of the automated test has no capitals in its commands.)* |
| `with readonly cwords -n 3 t t` | The counts doubled: `      6  the`, `      4  and`, `      2  bat`, then `16 words, 5 different` *(not tested)* |
| `with readonly cwords -n 5 cwords` | Five lines of short "words" found in the program's own machine code, then a total in the thousands — the same every time *(the automated test compares this with Linux's answer)* |

**Being told no**

| Type | You should see |
|---|---|
| `cwords -n none` | `cwords: -n needs a number from 1 up, not "none"`, then `cwords exited with code 2` |
| `cwords -n 0` | The same, with `"0"` *(not in the automated test)* |
| `cwords -n 12abc` | The same, with `"12abc"` *(not in the automated test)* |
| `cwords -n` | `usage: cwords [-n COUNT] [FILE...]`, then `cwords exited with code 2` *(not in the automated test)* |
| `cwords -x` | The usage line, then `cwords exited with code 2` |
| `with readonly cwords nofile t` | `cwords: nofile: no such file or directory`, then the counts for `t`, then `cwords exited with code 1` *(not in the automated test)* |

**The recipe** — on the host, with QEMU closed:

```
cat > libc/programs/mine.c <<'EOF'
#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>

static int backwards(const void *one, const void *other)
{
    long a = *(const long *)one, b = *(const long *)other;
    return (a < b) - (a > b);
}

int main(int argc, char **argv)
{
    long numbers[32];
    int count = 0;
    for (int n = 1; n < argc && count < 32; n++) {
        char *end;
        long number = strtol(argv[n], &end, 0);
        if (end == argv[n] || *end != '\0') {
            fprintf(stderr, "mine: \"%s\" is not a number\n", argv[n]);
            return 1;
        }
        numbers[count++] = number;
    }
    qsort(numbers, (size_t)count, sizeof numbers[0], backwards);
    for (int n = 0; n < count; n++)
        printf("%ld%c", numbers[n], n + 1 < count ? ' ' : '\n');
    return 0;
}
EOF
make cput C=mine
make run64
```

| Type | You should see |
|---|---|
| `mine 3 10 -4 7` | `10 7 3 -4` |
| `mine 0x10 010 10` | `16 10 8` — sixteen, ten and eight: base 0 reads `0x` as hex and a leading `0` as octal |
| `mine 5 five` | `mine: "five" is not a number`, then `mine exited with code 1` |
| `mine 12abc` | `mine: "12abc" is not a number`, then `mine exited with code 1` |

*(I ran `make cput` with a different five-line program, not this one: please try it.)* Delete `libc/programs/mine.c` afterwards, and `rm mine` in the OS.

The long way — several files, or a program kept somewhere else — is in `docs/c-programs.md`.

**On the host**

```
make libc-test
```

now runs five things: 10 string tests, 4 `errno` tests, `format: 123 checks, all the same as the host's printf`, `stdio: 124 checks, all passed`, and `stdlib: 7899 checks, all the same as the host's`.

```
build/c/host-cwords -n 3 docs/journal/042-a-real-c-program.md
```

runs the Linux build of the same `cwords.c` on this entry. *(Not tested on this file; the words will be the commonest in it.)*

**Cleaning up**, in the OS: `rm t u`.

Automated: `make test64`.
