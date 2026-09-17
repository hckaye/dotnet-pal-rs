/* Immutable synthetic metadata provider for cross-host negative contracts. */
#include "dotnet_pal.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
static int mode,calls;
_Alignas(8) static const unsigned char headers[56]={0};
static const unsigned char image_name[]="test-image";
static uint32_t iterate(dotnet_pal_image_visitor visitor,void *data,int32_t *out){
    dotnet_pal_image_view v={DOTNET_PAL_IMAGE_ELF64_LE,0,0,image_name,10,headers,1,7,3};
    if(mode==4)v.format=99;
    if(mode==5)v.header_count=65536;
    if(mode==6)v.headers=NULL;
    if(mode==7)v.name=NULL;
    int32_t result=visitor(&v,data);
    if(mode==8){visitor(&v,data);} /* illegally continue after nonzero visitor result */
    *out=mode==9?55:result;
    return mode==10?DOTNET_PAL_OS_ERROR:DOTNET_PAL_OK;
}
static uint32_t address(void *p,dotnet_pal_symbol_info *out){
    (void)p;
    *out=(dotnet_pal_symbol_info){(void*)0x1000,image_name,10,NULL,NULL,0};
    if(mode==11)out->base=NULL;
    if(mode==12)return DOTNET_PAL_NOT_FOUND;
    return DOTNET_PAL_OK;
}
static dotnet_pal_host_images table={
    {DOTNET_PAL_ABI_VERSION,sizeof(dotnet_pal_host_images),DOTNET_PAL_CAP_IMAGES},
    {iterate,address,NULL}
};
const dotnet_pal_host_images *dotnet_pal_host_images_v2(void){return &table;}
static int32_t callback(const dotnet_pal_image_view *v,void *p){
    assert(p==(void*)0x1234);calls++;assert(v->added==7 && v->removed==3);return -19;
}
int main(int argc,char **argv){
    assert(argc==2);mode=atoi(argv[1]);
    if(mode==1)table.header.abi_version=98;
    if(mode==2)table.header.struct_size=sizeof(dotnet_pal_header);
    if(mode==3)table.ops.address_info=NULL;
    const dotnet_pal_api *a=dotnet_pal_get_api(2);
    if(mode>=1 && mode<=3){assert(!a);puts("IMAGE HOST TABLE REJECTED");return 0;}
    assert(a && a->header.struct_size>=DOTNET_PAL_IMAGES_API_SIZE);
    int32_t result=99;
    if(mode<=10){
        uint32_t status=a->images.iterate(callback,(void*)0x1234,&result);
        if(mode==0){assert(status==0 && result==-19 && calls==1);}
        else{assert(status==DOTNET_PAL_OS_ERROR && result==0);if(mode<=7)assert(calls==0);}
    }else{
        dotnet_pal_symbol_info s={0};
        assert(a->images.address_info((void*)0x1111,&s)==(mode==11?DOTNET_PAL_OS_ERROR:DOTNET_PAL_NOT_FOUND));
        assert(!s.base && !s.name && !s.symbol_name);
    }
    assert(a->images.iterate(NULL,NULL,&result)==DOTNET_PAL_INVALID_ARGUMENT && result==0);
    puts("IMAGE CALLBACK/ERROR CONTRACT PASS");return 0;
}
