/* Disposable Linux FD inheritance probe. Compile into /var/tmp, not ~/.cargo/bin:
 * cc -Wall -Wextra -o /var/tmp/caller-writer-lease caller_writer_lease.c
 * /var/tmp/caller-writer-lease hold /var/tmp/<private-run>/writer.lock 5
 * (prints child PID; parent exits). In another shell, probe same path: busy
 * until child exits, then free. This is a kernel contract fixture, NOT host
 * launch admission. No installed service or native Pi is involved.
 */
#include <sys/file.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
int main(int argc, char **argv) {
    if (argc < 3 || (strcmp(argv[1], "hold") && strcmp(argv[1], "probe"))) return 2;
    int fd = open(argv[2], O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    struct stat st;
    if (fd < 0 || fstat(fd, &st) || !S_ISREG(st.st_mode) || (st.st_mode & 07777) != 0600 || st.st_nlink != 1) return 3;
    if (!strcmp(argv[1], "probe")) {
        if (flock(fd, LOCK_EX | LOCK_NB) == 0) { puts("free"); return 0; }
        if (errno == EWOULDBLOCK) { puts("busy"); return 1; }
        return 3;
    }
    if (argc != 4) return 2;
    unsigned long duration = strtoul(argv[3], NULL, 10);
    if (!duration || duration > 30 || flock(fd, LOCK_SH | LOCK_NB)) return 3;
    pid_t pid = fork();
    if (pid < 0) return 3;
    if (pid == 0) {
        /* Explicitly inherit this very open-file description through exec.
         * Parent exit alone must not drop its shared flock. */
        if (fcntl(fd, F_SETFD, 0)) _exit(3);
        char fd_arg[32], duration_arg[32];
        snprintf(fd_arg, sizeof(fd_arg), "%d", fd);
        snprintf(duration_arg, sizeof(duration_arg), "%lu", duration);
        execl("/bin/sh", "sh", "-c", "sleep \"$1\"; eval \"exec $2<&-\"", "sh", duration_arg, fd_arg, (char *)NULL);
        _exit(3);
    }
    printf("%ld\n", (long)pid);
    fflush(stdout);
    close(fd);
    return 0;
}
