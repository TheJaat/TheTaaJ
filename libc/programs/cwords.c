/* cwords - which words a text uses most.
 *
 *   cwords [-n COUNT] [FILE...]
 *
 * Reads the files, or standard input if there are none, and prints the
 * COUNT most used words (10 unless told otherwise) with how often each
 * appeared, and then how many words there were in all. A word is a run
 * of letters and digits; capitals and small letters count as the same.
 *
 *   with readonly cwords notes
 *   cat notes | cwords -n 3
 *
 * This is a program, not a test of the library: it is written the way
 * it would be for any system, in standard C and nothing else, and the
 * same file compiles and runs unchanged on the machine TheTaaJ is
 * built on. The automated test does exactly that, and compares what
 * the two print.
 *
 * How it works: every different word is kept once, with a count, in a
 * hash table - so that "have I seen this word?" does not mean looking
 * through all the words seen so far. At the end the words are put in
 * an array and sorted. */

#include <ctype.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* One different word. */
struct entry {
    char *word;
    long count;
    struct entry *next; /* the next word that landed in the same slot */
};

/* The table: an array of slots, each the start of a chain of entries.
 * A word's slot is worked out from its letters (hash), so looking a
 * word up means walking one short chain, not every word there is. */
static struct entry **slots;
static size_t slot_count;
static size_t different;
static long total;

static void no_memory(void)
{
    fprintf(stderr, "cwords: out of memory after %zu different words\n", different);
    exit(2);
}

/* A number made from the word's bytes, spread so that words alike in
 * spelling are not alike in number (the FNV-1a hash: for each byte,
 * mix it in and multiply by a prime). */
static unsigned long hash(const char *word)
{
    unsigned long number = 14695981039346656037ul;
    for (; *word != '\0'; word++) {
        number ^= (unsigned char)*word;
        number *= 1099511628211ul;
    }
    return number;
}

/* Twice the slots, and every entry moved to the slot it belongs in
 * now. Done whenever there are as many words as slots, so the chains
 * stay about one entry long however much text there is. */
static void grow(void)
{
    size_t new_count = slot_count == 0 ? 64 : slot_count * 2;
    struct entry **new_slots = calloc(new_count, sizeof *new_slots);
    if (new_slots == NULL)
        no_memory();
    for (size_t slot = 0; slot < slot_count; slot++) {
        struct entry *entry = slots[slot];
        while (entry != NULL) {
            struct entry *next = entry->next;
            size_t to = hash(entry->word) % new_count;
            entry->next = new_slots[to];
            new_slots[to] = entry;
            entry = next;
        }
    }
    free(slots);
    slots = new_slots;
    slot_count = new_count;
}

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
    struct entry *entry = malloc(sizeof *entry);
    char *copy = malloc(strlen(word) + 1);
    if (entry == NULL || copy == NULL)
        no_memory();
    strcpy(copy, word);
    entry->word = copy;
    entry->count = 1;
    entry->next = slots[slot];
    slots[slot] = entry;
    different++;
}

/* The words of one stream. A word may be any length: the room for it
 * grows as it does. */
static int count_stream(FILE *from)
{
    static char *word;
    static size_t room;
    size_t length = 0;
    for (;;) {
        int byte = getc(from);
        if (byte != EOF && isalnum(byte)) {
            if (length + 2 > room) {
                room = room == 0 ? 32 : room * 2;
                word = realloc(word, room);
                if (word == NULL)
                    no_memory();
            }
            word[length++] = (char)tolower(byte);
            continue;
        }
        if (length > 0) {
            word[length] = '\0';
            count_word(word);
            length = 0;
        }
        if (byte == EOF)
            return ferror(from) ? -1 : 0;
    }
}

/* The most used first; of two used equally, in the order of a
 * dictionary - so that the output is the same wherever this runs,
 * whatever order the table and the sort happen to leave them in. */
static int by_count(const void *one, const void *other)
{
    const struct entry *a = *(const struct entry *const *)one;
    const struct entry *b = *(const struct entry *const *)other;
    if (a->count != b->count)
        return a->count > b->count ? -1 : 1;
    return strcmp(a->word, b->word);
}

static void print_most(long most)
{
    struct entry **all = malloc((different + 1) * sizeof *all);
    if (all == NULL)
        no_memory();
    size_t found = 0;
    for (size_t slot = 0; slot < slot_count; slot++) {
        for (struct entry *entry = slots[slot]; entry != NULL; entry = entry->next)
            all[found++] = entry;
    }
    qsort(all, found, sizeof *all, by_count);
    for (size_t n = 0; n < found && (long)n < most; n++)
        printf("%7ld  %s\n", all[n]->count, all[n]->word);
    printf("%ld words, %zu different\n", total, different);
    free(all);
}

static int usage(void)
{
    fprintf(stderr, "usage: cwords [-n COUNT] [FILE...]\n");
    return 2;
}

int main(int argc, char **argv)
{
    long most = 10;
    int first = 1;
    if (argc > 1 && strcmp(argv[1], "-n") == 0) {
        if (argc < 3)
            return usage();
        char *end;
        errno = 0;
        most = strtol(argv[2], &end, 10);
        /* A number, all of it a number, that fits, and at least 1. */
        if (end == argv[2] || *end != '\0' || errno == ERANGE || most < 1) {
            fprintf(stderr, "cwords: -n needs a number from 1 up, not \"%s\"\n", argv[2]);
            return 2;
        }
        first = 3;
    } else if (argc > 1 && argv[1][0] == '-' && argv[1][1] != '\0') {
        return usage();
    }

    int failed = 0;
    if (first == argc) {
        if (count_stream(stdin) < 0) {
            fprintf(stderr, "cwords: standard input: %s\n", strerror(errno));
            failed = 1;
        }
    }
    for (int n = first; n < argc; n++) {
        FILE *file = fopen(argv[n], "r");
        if (file == NULL || count_stream(file) < 0) {
            fprintf(stderr, "cwords: %s: %s\n", argv[n], strerror(errno));
            failed = 1;
        }
        if (file != NULL)
            fclose(file);
    }
    print_most(most);
    return failed;
}
