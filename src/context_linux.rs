use super::*;
use core::sync::atomic::{AtomicI32,AtomicUsize,Ordering};
struct Slot {claimed:AtomicUsize,signal:AtomicI32,callback:AtomicUsize,data:AtomicUsize}
impl Slot {const fn new()->Self{Self{claimed:AtomicUsize::new(0),signal:AtomicI32::new(0),callback:AtomicUsize::new(0),data:AtomicUsize::new(0)}}}
static SLOTS:[Slot;5]=[const {Slot::new()};5];
extern "C" fn signal_number(kind:u32)->i32{signal(kind).unwrap_or(-1)}
fn signal(kind:u32)->Option<i32>{match kind{
    ACTIVATION=>Some(libc::SIGRTMIN()),SEGMENTATION=>Some(libc::SIGSEGV),BUS=>Some(libc::SIGBUS),
    FLOATING_POINT=>Some(libc::SIGFPE),ILLEGAL_INSTRUCTION=>Some(libc::SIGILL),_=>None}}
// Invoked by the kernel. No allocation, locking, API negotiation or Rust unwinding.
extern "C" fn trampoline(code:i32,info:*mut libc::siginfo_t,context:*mut c_void){
    let errno=unsafe{libc::__errno_location()};let saved=unsafe{errno.read()};
    for slot in &SLOTS {
        if slot.signal.load(Ordering::Acquire)==code {
            let address=slot.callback.load(Ordering::Acquire);
            if address!=0 {
                // Only an actual Callback supplied at registration can enter this slot.
                let callback:Callback=unsafe{mem::transmute(address)};
                unsafe{callback(code,info.cast(),context,slot.data.load(Ordering::Relaxed) as *mut c_void)};
            }
            break;
        }
    }
    unsafe{errno.write(saved)};
}
extern "C" fn abi_tag()->u64 {
    #[cfg(target_arch="x86_64")]return 0x4c4e580000000001;
    #[cfg(target_arch="aarch64")]return 0x4c4e580000000002;
    #[cfg(not(any(target_arch="x86_64",target_arch="aarch64")))]0
}
extern "C" fn action_size()->usize{mem::size_of::<libc::sigaction>()}
extern "C" fn action_alignment()->usize{mem::align_of::<libc::sigaction>()}
unsafe extern "C" fn install(kind:u32,callback:Option<Callback>,data:*mut c_void,previous:*mut c_void,_size:usize)->u32 {
    let Some(number)=signal(kind) else{return INVALID_ARGUMENT};let Some(callback)=callback else{return INVALID_ARGUMENT};
    let slot=&SLOTS[kind as usize];
    if slot.claimed.compare_exchange(0,1,Ordering::AcqRel,Ordering::Acquire).is_err(){return 6}
    let previous=previous.cast::<libc::sigaction>();
    // Publish the old action BEFORE enabling the callback: it may run immediately.
    if unsafe{libc::sigaction(number,ptr::null(),previous)}!=0 {slot.claimed.store(0,Ordering::Release);return OS_ERROR}
    let old=unsafe{previous.read()};
    if old.sa_sigaction==trampoline as *const () as usize {slot.claimed.store(0,Ordering::Release);return 6}
    let mut action:libc::sigaction=unsafe{mem::zeroed()};
    action.sa_flags=libc::SA_RESTART|libc::SA_SIGINFO;
    action.sa_sigaction=trampoline as *const () as usize;
    unsafe{libc::sigemptyset(&mut action.sa_mask)};
    if old.sa_flags & libc::SA_ONSTACK!=0 {action.sa_flags|=libc::SA_ONSTACK;action.sa_mask=old.sa_mask;}
    // Warm these libc entry points before a first asynchronous callback.
    unsafe{libc::__errno_location();libc::getpid();}
    slot.data.store(data as usize,Ordering::Relaxed);slot.callback.store(callback as usize,Ordering::Release);
    slot.signal.store(number,Ordering::Release);
    if unsafe{libc::sigaction(number,&action,ptr::null_mut())}!=0 {
        slot.signal.store(0,Ordering::Release);slot.callback.store(0,Ordering::Release);slot.claimed.store(0,Ordering::Release);return OS_ERROR;
    }
    OK
}
unsafe extern "C" fn restore(kind:u32,previous:*const c_void,_size:usize)->u32 {
    let Some(number)=signal(kind) else{return INVALID_ARGUMENT};
    if SLOTS[kind as usize].claimed.load(Ordering::Acquire)==0{return INVALID_ARGUMENT}
    if unsafe{libc::sigaction(number,previous.cast(),ptr::null_mut())}!=0{return OS_ERROR}
    // Keep immutable callback data for an already-running handler. Registrations
    // are process-lifetime; restoration does not make the slot reusable.
    OK
}
unsafe extern "C" fn unblock_activation()->u32 {
    let mut set:libc::sigset_t=unsafe{mem::zeroed()};
    unsafe{libc::sigemptyset(&mut set);libc::sigaddset(&mut set,libc::SIGRTMIN());}
    if unsafe{libc::pthread_sigmask(libc::SIG_UNBLOCK,&set,ptr::null_mut())}==0{OK}else{OS_ERROR}
}
unsafe extern "C" fn request_activation(token:usize)->u32 {
    match unsafe{libc::pthread_kill(token as libc::pthread_t,libc::SIGRTMIN())}{0=>OK,libc::EAGAIN=>6,libc::ESRCH=>8,_=>OS_ERROR}
}
unsafe extern "C" fn current_thread(out:*mut usize)->u32{unsafe{out.write(libc::pthread_self() as usize)};OK}
unsafe extern "C" fn process_id_async(out:*mut u64)->u32{unsafe{out.write(libc::getpid() as u64)};OK}
unsafe extern "C" fn ignore_broken_pipe()->u32 {
    let mut action:libc::sigaction=unsafe{mem::zeroed()};action.sa_sigaction=libc::SIG_IGN;
    unsafe{libc::sigemptyset(&mut action.sa_mask)};
    if unsafe{libc::sigaction(libc::SIGPIPE,&action,ptr::null_mut())}==0{OK}else{OS_ERROR}
}
static OPS:Ops=Ops {abi_tag:Some(abi_tag),action_size:Some(action_size),action_alignment:Some(action_alignment),
    install:Some(install),restore:Some(restore),unblock_activation:Some(unblock_activation),request_activation:Some(request_activation),
    current_thread:Some(current_thread),process_id_async:Some(process_id_async),ignore_broken_pipe:Some(ignore_broken_pipe),read_stats:None,signal_number:Some(signal_number)};
pub fn ops()->Option<&'static Ops>{Some(&OPS)}
