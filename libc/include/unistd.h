/* unistd.h - what the system itself does for a program.
 *
 * The functions are in libc.a (libc/src/lib.rs). A header only says
 * what they are called and what they take; the linker finds them. */
#ifndef _UNISTD_H
#define _UNISTD_H

#ifdef __cplusplus
extern "C" {
#endif

/* C11's _Noreturn is C++'s [[noreturn]]. */
#if defined(__cplusplus) && !defined(_Noreturn)
#define _Noreturn [[noreturn]]
#endif

#include <sys/types.h>

/* The descriptors every program starts with. */
#define STDIN_FILENO  0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

/* A descriptor is a small number standing for something open: one of
 * the three above, or a file that open (fcntl.h) returned. Each of
 * these returns -1 when it fails, with the reason in errno (errno.h). */

/* Writes count bytes to descriptor fd, where it has got to. Returns
 * how many. */
ssize_t write(int fd, const void *buffer, size_t count);

/* Reads up to count bytes. Returns how many: fewer than asked is not
 * the end; 0 is. */
ssize_t read(int fd, void *buffer, size_t count);

/* Done with the descriptor: its number may be given out again. */
int close(int fd);

/* Moves where the next read or write of a file starts: offset bytes
 * from the start, from where it is now, or from the end. Returns the
 * new place, counted from the start. */
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
off_t lseek(int fd, off_t offset, int whence);

/* Removes a file. */
int unlink(const char *path);

/* Sleeps for at least that many microseconds (nanosleep, time.h). */
int usleep(unsigned microseconds);

/* Names that do not start with '/' are taken from path from now on;
 * getcwd says it. For this program only. */
int chdir(const char *path);

/* The file becomes length bytes long: cut short, or with zeros added. */
int truncate(const char *path, off_t length);
int ftruncate(int fd, off_t length);

/* Links: TheTaaJ's files have one name each and there are no symbolic
 * links. link and symlink fail with EPERM; readlink with EINVAL ("not
 * a link") for anything that is there. */
int link(const char *from, const char *to);
int symlink(const char *target, const char *path);
ssize_t readlink(const char *path, char *buffer, size_t size);

/* Files have no owners: these change nothing, and succeed for what is
 * there. */
int chown(const char *path, uid_t owner, gid_t group);
int fchown(int fd, uid_t owner, gid_t group);

/* Removes an empty directory. */
int rmdir(const char *path);

/* May the program use what is at path so? F_OK: it is there; R_OK,
 * X_OK: it is there (a program given the files may read them all);
 * W_OK: and this program may change files. 0, or -1 with errno. */
#define F_OK 0
#define X_OK 1
#define W_OK 2
#define R_OK 4
int access(const char *path, int mode);

/* 1 if the descriptor is the screen; 0 if it is a file or another
 * program (errno ENOTTY), or not open at all (EBADF). */
int isatty(int fd);

/* Ends the program at once. */
_Noreturn void _exit(int code);

/* The directory the program was started in, as a path from the root,
 * into buffer (size bytes) - or, for NULL, into memory from malloc.
 * NULL, with errno ERANGE, if it does not fit. */
char *getcwd(char *buffer, size_t size);

/* The kernel's number for this process. */
pid_t getpid(void);
/* At most 256 random bytes from the kernel (sys/random.h has getrandom). */
int getentropy(void *buffer, size_t length);

/* Facts about the system: sysconf(_SC_PAGESIZE) is the size of a page
 * of memory (4096 here), _SC_NPROCESSORS_ONLN how many processors the
 * program can use (1: a program has one thread), _SC_CLK_TCK the
 * ticks of the clock in a second. -1, with EINVAL, for anything else.
 * The numbers are Linux's. */
#define _SC_ARG_MAX 0
#define _SC_CLK_TCK 2
#define _SC_OPEN_MAX 4
#define _SC_PAGESIZE 30
#define _SC_PAGE_SIZE _SC_PAGESIZE
#define _SC_NPROCESSORS_CONF 83
#define _SC_NPROCESSORS_ONLN 84
#define _SC_GETPW_R_SIZE_MAX 70

/* The least room for a program's arguments POSIX lets a system give. */
#define _POSIX_ARG_MAX 4096
long sysconf(int name);
int getpagesize(void);

/* SIGALRM in `seconds` (to the tick, 10 ms), in place of any alarm set
 * before; 0 only cancels. Returns the seconds the one before had left,
 * or 0. */
unsigned alarm(unsigned seconds);
/* Waits until a signal handler has run; always -1 with EINTR. */
int pause(void);

/* Replaces the program with another. TheTaaJ starts programs as new
 * processes and never replaces one: this always fails (ENOSYS). */
int execvp(const char *file, char *const argv[]);
int execv(const char *path, char *const argv[]);
int execve(const char *path, char *const argv[], char *const environment[]);

/* Copies the running program: not how TheTaaJ starts programs (a new
 * one is started from its file: posix_spawn, in spawn.h): always -1,
 * ENOSYS. */
pid_t fork(void);

/* The environment, "NAME=value" strings ending in NULL: what the
 * program that started it passed (spawn.h), and what it sets itself
 * (stdlib.h). */
extern char **environ;

/* The options in argv, one letter per call (POSIX): it stops at the
 * first word that is not an option, or "--", and never reorders. A
 * leading ':' in optstring: a missing argument is ':', and nothing is
 * printed; a leading '+' is skipped. optind = 0 starts again. */
extern char *optarg;
extern int optind, opterr, optopt;
int getopt(int argc, char *const argv[], const char *optstring);

/* The session a process is in: there are no sessions, so each process
 * is its own, and the answer is its number. */
pid_t getsid(pid_t process);

/* The user and group the program runs as: there are no users (pwd.h),
 * so always 0. */
uid_t getuid(void);
uid_t geteuid(void);
gid_t getgid(void);
gid_t getegid(void);

/* The machine's name: "taaj". */
int gethostname(char *name, size_t size);

/* A second descriptor for what fd is open on: the lowest free number
 * (dup), or `to`, closing what was there first (dup2). A file's copy
 * has its own place in the file - unlike Unix, where the two share
 * one - since the place is kept per descriptor here (fcntl.h). */
int dup(int fd);
int dup2(int fd, int to);

/* A pipe: fds[0] reads what fds[1] writes, in this program or - given
 * to one it starts (posix_spawn_file_actions_adddup2, spawn.h) - in
 * another. The reader sees the end once every write end is closed, in
 * every program that has one; a write with no reader left fails with
 * EPIPE. pipe2's O_CLOEXEC keeps both ends from the programs this one
 * starts (spawn.h); its O_NONBLOCK means nothing here - the ends wait. */
int pipe(int fds[2]);
int pipe2(int fds[2], int flags);

/* Writing out to the disk: every write already reaches the file
 * service before it returns, so these have nothing left to do. */
int fsync(int fd);

#ifdef __cplusplus
}
#endif

#endif
