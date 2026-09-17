#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static const dotnet_pal_support_ops *s;
static void *lock;
static atomic_int reader_ready,release_reader,writer_entered;
static unsigned total;
static volatile sig_atomic_t signal_ok;
static void *reader(void *unused){(void)unused;assert(s->rw_read(lock)==0);atomic_store(&reader_ready,1);
    while(!atomic_load(&release_reader)){sched_yield();}assert(s->rw_unlock(lock)==0);return NULL;}
static void *writer(void *unused){(void)unused;assert(s->rw_write(lock)==0);atomic_store(&writer_entered,1);assert(s->rw_unlock(lock)==0);return NULL;}
static void *worker(void *unused){
    (void)unused;assert(s->thread_name((const uint8_t*)"pal-support",11)==0);
    char name[32];assert(pthread_getname_np(pthread_self(),name,sizeof name)==0);assert(!strcmp(name,"pal-support"));
    for(int i=0;i<500;i++){
        void *p=NULL,*q=NULL;assert(s->allocate(37,1,&p)==0);
        for(size_t n=0;n<37;n++){assert(((uint8_t*)p)[n]==0);}
        memset(p,0x5a,37);assert(s->resize(p,117,&q)==0);
        for(size_t n=0;n<37;n++){assert(((uint8_t*)q)[n]==0x5a);}
        assert(s->release(q)==0);
        assert(s->rw_write(lock)==0);total++;assert(s->rw_unlock(lock)==0);
    }return NULL;
}
static void handler(int sig){(void)sig;size_t done=0;
    signal_ok=s->write_stderr((const uint8_t*)"signal\n",7,&done)==0 && done==7;}
int main(void){
    const dotnet_pal_api *api=dotnet_pal_get_api(2);assert(api);
    assert(api->header.struct_size>=DOTNET_PAL_SUPPORT_API_SIZE);
    assert((api->header.capabilities&DOTNET_PAL_CAP_SUPPORT)==DOTNET_PAL_CAP_SUPPORT);s=&api->support;
    void *p=(void*)1;assert(s->allocate(0,0,&p)==DOTNET_PAL_INVALID_ARGUMENT && !p);
    assert(s->allocate(SIZE_MAX,0,&p)==DOTNET_PAL_INVALID_ARGUMENT && !p);
    assert(s->allocate(1,2,&p)==DOTNET_PAL_INVALID_ARGUMENT && !p);
    assert(s->allocate(1,0,NULL)==DOTNET_PAL_INVALID_ARGUMENT);
    assert(s->resize(NULL,0,&p)==DOTNET_PAL_INVALID_ARGUMENT && !p);
    assert(s->release(NULL)==0);assert(s->rw_read(NULL)==DOTNET_PAL_INVALID_ARGUMENT);
    assert(s->rw_create(&lock)==0);assert(s->rw_read(lock)==0);
    pthread_t a,b;assert(!pthread_create(&a,NULL,reader,NULL));
    while(!atomic_load(&reader_ready)){sched_yield();} /* requires genuinely shared reads */
    assert(!pthread_create(&b,NULL,writer,NULL));assert(!atomic_load(&writer_entered));
    assert(s->rw_unlock(lock)==0);assert(!atomic_load(&writer_entered));
    atomic_store(&release_reader,1);assert(!pthread_join(a,NULL));assert(!pthread_join(b,NULL));assert(atomic_load(&writer_entered));
    pthread_t workers[4];for(int i=0;i<4;i++){assert(!pthread_create(&workers[i],NULL,worker,NULL));}
    for(int i=0;i<4;i++){assert(!pthread_join(workers[i],NULL));}assert(total==2000);
    assert(s->rw_destroy(lock)==0);
    int fds[2],saved=dup(2);assert(saved>=0 && pipe(fds)==0);assert(dup2(fds[1],2)==2);close(fds[1]);
    size_t done=99;assert(s->write_stderr((const uint8_t*)"normal\n",7,&done)==0 && done==7);
    struct sigaction action={0},old={0};action.sa_handler=handler;sigemptyset(&action.sa_mask);
    assert(!sigaction(SIGUSR1,&action,&old));assert(!raise(SIGUSR1));assert(signal_ok);assert(!sigaction(SIGUSR1,&old,NULL));
    assert(dup2(saved,2)==2);close(saved);char result[32]={0};assert(read(fds[0],result,sizeof result)==14);close(fds[0]);
    assert(!strcmp(result,"normal\nsignal\n"));
    assert(s->write_stderr(NULL,1,&done)==DOTNET_PAL_INVALID_ARGUMENT && done==0);
    assert(s->write_stderr(NULL,0,&done)==0 && done==0);
    assert(s->thread_name((const uint8_t*)"a\0b",3)==DOTNET_PAL_INVALID_ARGUMENT);
    dotnet_pal_support_stats stats={0};assert(s->read_stats(&stats,sizeof stats)==0);
    assert(stats.allocate_ok==2000 && stats.resize_ok==2000 && stats.release_ok==2001);
    assert(stats.rw_read_ok==2 && stats.rw_write_ok==2001 && stats.rw_destroy_ok==1);
    assert(stats.write_ok==3 && stats.name_ok==4);
    puts("NATIVE SUPPORT PASS heap/resize, shared readers, exclusive writers, concurrency, signal diagnostics, names");return 0;
}
