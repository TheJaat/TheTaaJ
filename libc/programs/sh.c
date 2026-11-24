/* sh.c - the shell programs start, as POSIX's system and popen do, and
 * make for each recipe:
 *
 *   sh [-ex] -c COMMAND [NAME [ARGUMENT...]]    runs COMMAND
 *   sh [-ex] FILE [ARGUMENT...]                 runs the script in FILE
 *
 * Lists (; && ||), pipelines, redirections, variables, quoting,
 * wildcards, $(...), and the builtins a recipe uses. The language is in
 * the C library (libc/src/sh.rs), where it is tested on the host; this
 * is only the program around it. */

int __taaj_sh(int argc, char **argv);

int main(int argc, char **argv)
{
    return __taaj_sh(argc, argv);
}
