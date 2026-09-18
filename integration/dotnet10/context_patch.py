"""Preserve the upstream hijack/hardware-EH algorithms, route native OS services.

Opaque signal info and CPU contexts are specific to the agreed Linux SDK ABI;
this patch does not pretend Rust's target list supplies codegen/unwind metadata.
"""
from kernel_patch import once
from runtime_patch import function as runtime_function
SIGNALS='src/coreclr/nativeaot/Runtime/unix/UnixSignals.cpp'
HEADER='src/coreclr/nativeaot/Runtime/unix/UnixSignals.h'
THREAD='src/coreclr/nativeaot/Runtime/thread.cpp'
GC_STRUCTS='src/coreclr/gc/env/gcenv.structs.h'
FILES=(SIGNALS,HEADER,THREAD,GC_STRUCTS)
def guarded(original,replacement):
    return '#ifdef DOTNET_PAL_NATIVE_CONTEXT\n'+replacement+'\n#else\n'+original+'\n#endif'
def function(text,name,body):
    # Reuse the fully matched function edit, then only rename its fresh guards.
    placeholder='DOTNET_PAL_CONTEXT_FRESH'
    text=text.replace('DOTNET_PAL_RUNTIME',placeholder)
    out=runtime_function(text,name,body).replace('DOTNET_PAL_RUNTIME','DOTNET_PAL_NATIVE_CONTEXT')
    return out.replace(placeholder,'DOTNET_PAL_RUNTIME')
def header(text):
    old='#ifdef SIGRTMIN\n#define INJECT_ACTIVATION_SIGNAL SIGRTMIN\n#else\n#define INJECT_ACTIVATION_SIGNAL SIGUSR1\n#endif'
    new='#include "context_adapter.h"\n#define INJECT_ACTIVATION_SIGNAL (dotnet_pal_context::require()->signal_number(DOTNET_PAL_SIGNAL_ACTIVATION))'
    return once(text,old,guarded(old,new))
def signals(text):
    text=function(text,'AddSignalHandler','    return dotnet_pal_context::add(signal,handler,previousAction);')
    return function(text,'RestoreSignalHandler','    dotnet_pal_context::restore(signal_id,previousAction);')
def thread(text):
    old='    m_hOSThread = pthread_self();'
    text=once(text,old,guarded(old,'    m_hOSThread = static_cast<pthread_t>(dotnet_pal_context::thread_token());'))
    return '#ifdef DOTNET_PAL_NATIVE_CONTEXT\n#include "context_adapter.h"\n#endif\n'+text
def pal(text):
    text=once(text,'bool PalInit()\n{','bool PalInit()\n{\n#ifdef DOTNET_PAL_NATIVE_CONTEXT\n    if (!dotnet_pal_context::initialize()) return false;\n#endif')
    text=function(text,'UnmaskActivationSignal','    dotnet_pal_context::unblock();')
    text=function(text,'ConfigureSignals','    dotnet_pal_context::configure();')
    old='    int status = pthread_kill(pThreadToHijack->GetOSThreadHandle(), INJECT_ACTIVATION_SIGNAL);'
    text=once(text,old,guarded(old,'    int status = dotnet_pal_context::request(static_cast<uintptr_t>(pThreadToHijack->GetOSThreadHandle()));'))
    old='        if (siginfo->si_pid == getpid()'
    text=once(text,old,guarded(old,'        if (siginfo->si_pid == static_cast<pid_t>(dotnet_pal_context::process_id_async())'))
    # Verify the serviced inline-suspension and previous-handler chaining survive.
    for required in ['GetCurrentThreadIfAvailableAsyncSafe','doInlineSuspend','g_previousActivationHandler.sa_sigaction(code, siginfo, context)']:
        if required not in text:raise ValueError('serviced activation semantics missing: '+required)
    return '#ifdef DOTNET_PAL_NATIVE_CONTEXT\n#include "context_adapter.h"\n#endif\n'+text
def gc_structs(text):
    if 'dotnet_pal::ThreadIdentity' in text or text.count('class EEThreadId\n{') != 2:
        raise ValueError('GC thread identity definitions changed or were already patched')
    # Replace only the Unix definition; preserve the Windows class and all
    # non-NativeAOT users of this shared GC header.
    begin=text.index('class EEThreadId\n{',text.index('#ifdef TARGET_UNIX'))
    end=text.index('\n};',begin)+len('\n};')
    original=text[begin:end]
    if original.count('pthread_self()')!=2 or original.count('pthread_equal(')!=1:
        raise ValueError('GC thread identity call sites changed')
    replacement='''#include "context_adapter.h"
#include "thread_identity.h"
class EEThreadId final : public dotnet_pal::ThreadIdentity<dotnet_pal_context::thread_token> {};'''
    return text[:begin]+guarded(original,replacement)+text[end:]
TRANSFORMS={SIGNALS:signals,HEADER:header,THREAD:thread,GC_STRUCTS:gc_structs}
