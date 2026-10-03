#include <sys/socket.h>
#include <sys/ioctl.h>
#include <net/if.h>
#include <string.h>
#include <stdio.h>
int main(void) {
 int fd=socket(AF_INET, SOCK_DGRAM, 0);
 struct ifreq req; memset(&req,0,sizeof(req)); strcpy(req.ifr_name,"lo");
 if(ioctl(fd,SIOCGIFFLAGS,&req)<0) return 1;
 req.ifr_flags |= IFF_UP|IFF_RUNNING;
 if(ioctl(fd,SIOCSIFFLAGS,&req)<0) return 1;
 return 0;
}
