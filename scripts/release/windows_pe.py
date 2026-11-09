"""Inspect ordinary and delayed PE imports without relying on a runner's PATH."""
import pathlib
import struct

SYSTEM_DLLS = set('''advapi32 avrt bcrypt bcryptprimitives cabinet cfgmgr32 clbcatq combase comctl32 comdlg32 crypt32 cryptbase cryptsp d2d1 d3d9 d3d11 d3d12 d3dcompiler_47 dbghelp dcomp dnsapi dwmapi dwrite dxgi gdi32 gdi32full hid imagehlp imm32 iphlpapi kernel32 kernelbase mmdevapi mpr msacm32 msasn1 mscms msctf msimg32 msvcp_win msvcrt mswsock ncrypt netapi32 normaliz ntdll ole32 oleacc oleaut32 opengl32 powrprof profapi propsys psapi rpcrt4 rtworkq secur32 setupapi shcore shell32 shfolder shlwapi srvcli sspicli ucrtbase user32 userenv usp10 uxtheme version wer windowscodecs winhttp wininet winmm winspool wintrust winusb wldap32 ws2_32 wtsapi32'''.split())

# AccessKit uses the Windows UI Automation provider API.
SYSTEM_DLLS.add('uiautomationcore')

def imports(binary):
    data=pathlib.Path(binary).read_bytes()
    def uint(offset,fmt):
        if offset<0 or offset+struct.calcsize(fmt)>len(data):
            raise ValueError('truncated PE structure')
        return struct.unpack_from(fmt,data,offset)[0]
    if data[:2]!=b'MZ':raise ValueError('expected PE executable')
    pe=uint(0x3c,'<I')
    if data[pe:pe+4]!=b'PE\0\0':raise ValueError('invalid PE signature')
    count=uint(pe+6,'<H');optional_size=uint(pe+20,'<H');optional=pe+24
    magic=uint(optional,'<H')
    if magic==0x20b:
        directories=optional+112;number=uint(optional+108,'<I');image_base=uint(optional+24,'<Q')
    elif magic==0x10b:
        directories=optional+96;number=uint(optional+92,'<I');image_base=uint(optional+28,'<I')
    else:raise ValueError('unsupported PE optional header')
    if count>96 or optional_size<(directories-optional)+min(number,16)*8:
        raise ValueError('invalid PE section/directory count')
    section_table=optional+optional_size
    def offset(rva):
        for i in range(count):
            section=section_table+i*40
            size=uint(section+8,'<I');address=uint(section+12,'<I')
            raw_size=uint(section+16,'<I');raw=uint(section+20,'<I')
            if address<=rva<address+max(size,raw_size):
                if rva-address>=raw_size:raise ValueError('PE import points outside file-backed section')
                result=raw+rva-address
                if result>=len(data):raise ValueError('PE import points beyond executable')
                return result
        if rva<uint(optional+60,'<I') and rva<len(data):return rva
        raise ValueError('unmapped PE import address')
    def dll_name(rva):
        begin=offset(rva);end=data.find(b'\0',begin,min(begin+261,len(data)))
        if end<0:raise ValueError('unbounded PE import name')
        try:name=data[begin:end].decode('ascii').lower()
        except UnicodeDecodeError:raise ValueError('non-ASCII PE import name')
        if not name or '/' in name or '\\' in name or ':' in name:raise ValueError('invalid PE import name')
        return name
    result=set()
    for directory,stride,name_offset,delayed in ((1,20,12,False),(13,32,4,True)):
        if directory>=number:continue
        rva=uint(directories+directory*8,'<I');size=uint(directories+directory*8+4,'<I')
        if not rva:continue
        if size>1024*1024:raise ValueError('PE import directory budget exceeded')
        start=offset(rva)
        for n in range(4096):
            entry=start+n*stride
            if entry+stride>len(data):raise ValueError('truncated PE import descriptor')
            if not any(data[entry:entry+stride]):break
            name=uint(entry+name_offset,'<I')
            if delayed and not uint(entry,'<I')&1:name-=image_base
            result.add(dll_name(name))
        else:raise ValueError('unterminated PE import directory')
    return sorted(result)

def verify(binary):
    names=imports(binary)
    unknown = [
        name for name in names
        if not name.endswith('.dll') or not (
            name[:-4] in SYSTEM_DLLS or name.startswith(('api-ms-win-', 'ext-ms-win-'))
        )
    ]
    if unknown:
        raise ValueError('unbundled non-system Windows runtime dependency: '+', '.join(unknown))
    return names
