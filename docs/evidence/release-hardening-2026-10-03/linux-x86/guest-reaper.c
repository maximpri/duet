#include <sys/wait.h>
#include <unistd.h>
#include <errno.h>
#include <sys/reboot.h>
int main(void) {
 pid_t child=fork();
 if(child==0) { execl("/bin/bash", "bash", "/duet-init", (char*)0); _exit(127); }
 int status=0;
 while(1) { pid_t done=wait(&status); if(done<0 && errno==EINTR) continue; if(done==child || done<0) break; }
 sync(); reboot(RB_POWER_OFF); return 1;
}
