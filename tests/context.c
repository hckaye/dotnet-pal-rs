#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>
#ifdef PAL_CONTEXT_HOST
extern int pal_context_fault;
#endif
static const dotnet_pal_context_ops *c;
static _Atomic unsigned hits,old_hits,invalid;
static unsigned cookie=0x1234;
static struct sigaction previous, original, previous_fpe;
static void old_handler(int code){(void)code;atomic_fetch_add(&old_hits,1);}
static void callback(int32_t code,void *info,void *context,void *data){
    siginfo_t *signal=info;uint64_t id=0;
    if(data!=&cookie || cookie!=0x1234 || !signal || !context || signal->si_signo!=code ||
        c->process_id_async(&id)!=0 || id!=(uint64_t)getpid())atomic_fetch_add(&invalid,1);
    if(code==c->signal_number(DOTNET_PAL_SIGNAL_ACTIVATION)) {
        sigset_t mask;
        if(sigprocmask(SIG_SETMASK,NULL,&mask)!=0 || !sigismember(&mask,SIGUSR2))atomic_fetch_add(&invalid,1);
        previous.sa_handler(code); // the native adapter owns previous-handler chaining
    }
    errno=ERANGE;
    atomic_fetch_add(&hits,1);
}
static void *worker(void *unused){
    (void)unused;uintptr_t token=0;
    assert(c->current_thread(&token)==0 && token==(uintptr_t)pthread_self());
    assert(c->unblock_activation()==0);
    for(int i=0;i<500;++i){errno=E2BIG;assert(c->request_activation(token)==0);assert(errno==E2BIG);}
    return NULL;
}
int main(int argc,char **argv){
    (void)argc;(void)argv;
#ifdef PAL_CONTEXT_HOST
    pal_context_fault=argc>1?atoi(argv[1]):0;
#endif
    const dotnet_pal_api *a=dotnet_pal_get_api(2);
#ifdef PAL_CONTEXT_HOST
    if(pal_context_fault){assert(!a);puts("CONTEXT malformed host table rejected");return 0;}
#endif
    assert(a && a->header.struct_size>=DOTNET_PAL_CONTEXT_API_SIZE && (a->header.capabilities&DOTNET_PAL_CAP_NATIVE_CONTEXT));
    c=&a->context;
    assert(c->action_size()==sizeof(struct sigaction) && c->action_alignment()==_Alignof(struct sigaction));
#if defined(__x86_64__)
    assert(c->abi_tag()==DOTNET_PAL_CONTEXT_LINUX_X64);
#else
    assert(c->abi_tag()==DOTNET_PAL_CONTEXT_LINUX_ARM64);
#endif
    assert(c->signal_number(DOTNET_PAL_SIGNAL_ACTIVATION)==SIGRTMIN);
    assert(c->signal_number(99)==-1);
    assert(c->install(99,callback,&cookie,&previous,sizeof previous)==DOTNET_PAL_INVALID_ARGUMENT);
    assert(c->install(0,NULL,&cookie,&previous,sizeof previous)==DOTNET_PAL_INVALID_ARGUMENT);
    assert(c->install(0,callback,&cookie,&previous,sizeof previous-1)==DOTNET_PAL_INVALID_ARGUMENT);
    assert(c->current_thread(NULL)==DOTNET_PAL_INVALID_ARGUMENT);
    struct sigaction before={0};before.sa_handler=old_handler;before.sa_flags=SA_ONSTACK;
    sigemptyset(&before.sa_mask);sigaddset(&before.sa_mask,SIGUSR2);
    assert(sigaction(SIGRTMIN,&before,&original)==0);
    assert(c->install(0,callback,&cookie,&previous,sizeof previous)==0);
    assert(previous.sa_handler==old_handler);
    struct sigaction observed;assert(sigaction(SIGRTMIN,NULL,&observed)==0);
    assert((observed.sa_flags&(SA_RESTART|SA_SIGINFO|SA_ONSTACK))==(SA_RESTART|SA_SIGINFO|SA_ONSTACK));
    assert(sigismember(&observed.sa_mask,SIGUSR2));
    assert(c->install(0,callback,&cookie,&previous,sizeof previous)==DOTNET_PAL_BUSY);
    assert(c->install(3,callback,&cookie,&previous_fpe,sizeof previous_fpe)==0);
    errno=E2BIG;assert(raise(SIGFPE)==0);assert(errno==E2BIG);
    pthread_t workers[4];
    for(int i=0;i<4;++i)assert(pthread_create(&workers[i],NULL,worker,NULL)==0);
    for(int i=0;i<4;++i)assert(pthread_join(workers[i],NULL)==0);
    assert(atomic_load(&hits)==2001 && atomic_load(&old_hits)==2000 && atomic_load(&invalid)==0);
    assert(c->restore(0,&previous,sizeof previous)==0);
    assert(raise(SIGRTMIN)==0 && atomic_load(&old_hits)==2001);
    assert(c->restore(3,&previous_fpe,sizeof previous_fpe)==0);
    assert(sigaction(SIGRTMIN,&original,NULL)==0);
    // Restoring does not reclaim callback storage while a handler may still exist.
    assert(c->install(0,callback,&cookie,&previous,sizeof previous)==DOTNET_PAL_BUSY);
    pid_t child=fork();assert(child>=0);
    if(child==0){
        assert(c->ignore_broken_pipe()==0);int fds[2];assert(pipe(fds)==0);assert(close(fds[0])==0);
        errno=0;assert(write(fds[1],"x",1)==-1 && errno==EPIPE);assert(close(fds[1])==0);_exit(0);
    }
    int status=0;assert(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==0);
    dotnet_pal_context_stats stats;assert(c->read_stats(&stats,sizeof stats)==0);
    assert(stats.installs==2 && stats.requests==2000 && stats.unblocks==4 && stats.restores==2);
    puts("NATIVE CONTEXT PASS ABI sizes, real signals, mask/altstack preservation, chaining, errno, concurrent activations, restore");
}
