// SPDX-License-Identifier: GPL-2.0-or-later
//! ossteam's own `steamclient.dll` and `steamclient64.dll` for Proton
//! prefixes (`compat-tools.md` §6.3).
//!
//! Proton's Wine loads the file the prefix's registry names, then writes
//! a jump to Proton's `lsteamclient` over each export it shares with it.
//! So the files only have to exist as DLLs with the right export names;
//! each export's own code returns 0, which is what runs when a name is
//! not redirected.

use std::io;
use std::path::Path;

/// The export names of the official client's `steamclient.dll` and
/// `steamclient64.dll`, in the byte order the export table needs.
pub const EXPORTS: [&str; 41] = [
    "Breakpad_SteamMiniDumpInit",
    "Breakpad_SteamSendMiniDump",
    "Breakpad_SteamSetAppID",
    "Breakpad_SteamSetSteamID",
    "Breakpad_SteamWriteMiniDumpSetComment",
    "Breakpad_SteamWriteMiniDumpUsingExceptionInfoWithBuildId",
    "CreateInterface",
    "Steam_BConnected",
    "Steam_BGetCallback",
    "Steam_BLoggedOn",
    "Steam_BReleaseSteamPipe",
    "Steam_ConnectToGlobalUser",
    "Steam_CreateGlobalUser",
    "Steam_CreateLocalUser",
    "Steam_CreateSteamPipe",
    "Steam_FreeLastCallback",
    "Steam_GSBLoggedOn",
    "Steam_GSBSecure",
    "Steam_GSGetSteam2GetEncryptionKeyToSendToNewClient",
    "Steam_GSGetSteamID",
    "Steam_GSLogOff",
    "Steam_GSLogOn",
    "Steam_GSRemoveUserConnect",
    "Steam_GSSendSteam2UserConnect",
    "Steam_GSSendSteam3UserConnect",
    "Steam_GSSendUserDisconnect",
    "Steam_GSSendUserStatusResponse",
    "Steam_GSSetServerType",
    "Steam_GSSetSpawnCount",
    "Steam_GSUpdateStatus",
    "Steam_GetAPICallResult",
    "Steam_GetGSHandle",
    "Steam_InitiateGameConnection",
    "Steam_IsKnownInterface",
    "Steam_LogOff",
    "Steam_LogOn",
    "Steam_NotifyMissingInterface",
    "Steam_ReleaseThreadLocalMemory",
    "Steam_ReleaseUser",
    "Steam_SetLocalIPBinding",
    "Steam_TerminateGameConnection",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    /// `steamclient.dll`, PE32.
    X86,
    /// `steamclient64.dll`, PE32+.
    X64,
}

impl Arch {
    pub fn file_name(self) -> &'static str {
        match self {
            Arch::X86 => "steamclient.dll",
            Arch::X64 => "steamclient64.dll",
        }
    }

    fn image_base(self) -> u64 {
        match self {
            Arch::X86 => 0x3800_0000,
            Arch::X64 => 0x1_8000_0000,
        }
    }
}

/// Each function's size: room for Proton's 12-byte jump.
pub const STUB: usize = 16;
const SECTION_ALIGN: u32 = 0x1000;
const FILE_ALIGN: u32 = 0x200;
const TEXT_RVA: u32 = SECTION_ALIGN;
const PE_OFFSET: usize = 0x40;

fn align(n: u32, to: u32) -> u32 {
    n.div_ceil(to) * to
}

/// The code: the entry point, then one function per export.
fn text(arch: Arch) -> Vec<u8> {
    // mov eax, 1; then ret 12 (stdcall DllMain) or ret.
    let entry: &[u8] = match arch {
        Arch::X86 => &[0xB8, 1, 0, 0, 0, 0xC2, 12, 0],
        Arch::X64 => &[0xB8, 1, 0, 0, 0, 0xC3],
    };
    // xor eax, eax; ret (cdecl on i386; clears rax on x86_64).
    let export: &[u8] = &[0x31, 0xC0, 0xC3];
    let mut out = Vec::with_capacity(STUB * (1 + EXPORTS.len()));
    for code in std::iter::once(entry).chain(EXPORTS.iter().map(|_| export)) {
        let start = out.len();
        out.extend_from_slice(code);
        out.resize(start + STUB, 0xCC);
    }
    out
}

/// The export directory, placed at `rva`.
fn edata(arch: Arch, rva: u32) -> Vec<u8> {
    let n = EXPORTS.len() as u32;
    let functions = rva + 40;
    let names = functions + 4 * n;
    let ordinals = names + 4 * n;
    let strings = ordinals + 2 * n;

    let mut text = Vec::new();
    let dll_name = strings;
    text.extend_from_slice(arch.file_name().as_bytes());
    text.push(0);
    let mut name_rvas = Vec::new();
    for name in EXPORTS {
        name_rvas.push(strings + text.len() as u32);
        text.extend_from_slice(name.as_bytes());
        text.push(0);
    }

    let mut out = Vec::new();
    for v in [0u32, 0, 0, dll_name, 1, n, n, functions, names, ordinals] {
        // Characteristics, TimeDateStamp, Major/MinorVersion (one u32),
        // Name, Base, NumberOfFunctions, NumberOfNames, the three tables.
        out.extend_from_slice(&v.to_le_bytes());
    }
    for i in 0..n {
        let f = TEXT_RVA + STUB as u32 * (i + 1);
        out.extend_from_slice(&f.to_le_bytes());
    }
    for r in name_rvas {
        out.extend_from_slice(&r.to_le_bytes());
    }
    for i in 0..n as u16 {
        out.extend_from_slice(&i.to_le_bytes());
    }
    out.extend_from_slice(&text);
    out
}

struct Section {
    name: &'static [u8],
    data: Vec<u8>,
    flags: u32,
}

/// The DLL for `arch` (`compat-tools.md` §6.3).
pub fn image(arch: Arch) -> Vec<u8> {
    const CODE: u32 = 0x6000_0020; // code, execute, read
    const DATA: u32 = 0x4000_0040; // initialized data, read
    const RELOC: u32 = 0x4200_0040; // initialized data, discardable, read

    let text = text(arch);
    let edata_rva = TEXT_RVA + align(text.len() as u32, SECTION_ALIGN);
    let edata = edata(arch, edata_rva);
    let reloc_rva = edata_rva + align(edata.len() as u32, SECTION_ALIGN);
    // One block for the code's page, with two padding entries
    // (IMAGE_REL_BASED_ABSOLUTE): nothing to fix up.
    let mut reloc = Vec::new();
    reloc.extend_from_slice(&TEXT_RVA.to_le_bytes());
    reloc.extend_from_slice(&12u32.to_le_bytes());
    reloc.extend_from_slice(&[0; 4]);
    let edata_len = edata.len() as u32;
    let reloc_len = reloc.len() as u32;
    let sections = [
        Section {
            name: b".text",
            data: text,
            flags: CODE,
        },
        Section {
            name: b".edata",
            data: edata,
            flags: DATA,
        },
        Section {
            name: b".reloc",
            data: reloc,
            flags: RELOC,
        },
    ];

    let pe32plus = arch == Arch::X64;
    let optional_len: u16 = if pe32plus { 240 } else { 224 };
    let headers_len = PE_OFFSET + 4 + 20 + optional_len as usize + 40 * sections.len();
    let headers_size = align(headers_len as u32, FILE_ALIGN);
    let image_size = reloc_rva + align(reloc_len, SECTION_ALIGN);
    let raw = |s: &Section| align(s.data.len() as u32, FILE_ALIGN);

    let mut out = vec![0u8; PE_OFFSET];
    out[0..2].copy_from_slice(b"MZ");
    out[0x3C..0x40].copy_from_slice(&(PE_OFFSET as u32).to_le_bytes());
    out.extend_from_slice(b"PE\0\0");

    // COFF header.
    let (machine, characteristics): (u16, u16) = match arch {
        // executable image, 32-bit machine, DLL
        Arch::X86 => (0x014C, 0x2102),
        // executable image, large address aware, DLL
        Arch::X64 => (0x8664, 0x2022),
    };
    let put16 = |out: &mut Vec<u8>, v: u16| out.extend_from_slice(&v.to_le_bytes());
    let put32 = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    put16(&mut out, machine);
    put16(&mut out, sections.len() as u16);
    put32(&mut out, 0); // TimeDateStamp: the same bytes every time
    put32(&mut out, 0); // PointerToSymbolTable
    put32(&mut out, 0); // NumberOfSymbols
    put16(&mut out, optional_len);
    put16(&mut out, characteristics);

    // Optional header.
    let optional_start = out.len();
    put16(&mut out, if pe32plus { 0x20B } else { 0x10B });
    out.extend_from_slice(&[0, 0]); // linker version
    put32(&mut out, raw(&sections[0])); // SizeOfCode
    put32(&mut out, raw(&sections[1]) + raw(&sections[2])); // SizeOfInitializedData
    put32(&mut out, 0); // SizeOfUninitializedData
    put32(&mut out, TEXT_RVA); // AddressOfEntryPoint
    put32(&mut out, TEXT_RVA); // BaseOfCode
    if pe32plus {
        out.extend_from_slice(&arch.image_base().to_le_bytes());
    } else {
        put32(&mut out, edata_rva); // BaseOfData
        put32(&mut out, arch.image_base() as u32);
    }
    put32(&mut out, SECTION_ALIGN);
    put32(&mut out, FILE_ALIGN);
    for v in [6u16, 0, 0, 0, 6, 0] {
        // operating system, image and subsystem versions
        put16(&mut out, v);
    }
    put32(&mut out, 0); // Win32VersionValue
    put32(&mut out, image_size);
    put32(&mut out, headers_size);
    put32(&mut out, 0); // CheckSum
    put16(&mut out, 2); // Subsystem: Windows GUI
    put16(&mut out, 0x0140); // DllCharacteristics: dynamic base, NX compatible
    for v in [0x10_0000u64, 0x1000, 0x10_0000, 0x1000] {
        // stack and heap reserve and commit
        if pe32plus {
            out.extend_from_slice(&v.to_le_bytes());
        } else {
            put32(&mut out, v as u32);
        }
    }
    put32(&mut out, 0); // LoaderFlags
    put32(&mut out, 16); // NumberOfRvaAndSizes
    for i in 0..16 {
        let (rva, len) = match i {
            0 => (edata_rva, edata_len),
            5 => (reloc_rva, reloc_len),
            _ => (0, 0),
        };
        put32(&mut out, rva);
        put32(&mut out, len);
    }
    debug_assert_eq!(out.len() - optional_start, optional_len as usize);

    // Section headers, then the sections, each at a file-aligned offset.
    let mut rva = TEXT_RVA;
    let mut offset = headers_size;
    for s in &sections {
        let mut name = [0u8; 8];
        name[..s.name.len()].copy_from_slice(s.name);
        out.extend_from_slice(&name);
        put32(&mut out, s.data.len() as u32); // VirtualSize
        put32(&mut out, rva);
        put32(&mut out, raw(s));
        put32(&mut out, offset);
        out.extend_from_slice(&[0; 12]); // relocations and line numbers
        put32(&mut out, s.flags);
        rva += align(s.data.len() as u32, SECTION_ALIGN);
        offset += raw(s);
    }
    for s in &sections {
        out.resize(align(out.len() as u32, FILE_ALIGN) as usize, 0);
        out.extend_from_slice(&s.data);
    }
    out.resize(align(out.len() as u32, FILE_ALIGN) as usize, 0);
    out
}

/// Writes both DLLs into `<client_dir>/legacycompat/`, each only when
/// its content differs, so that an unchanged file keeps its
/// modification time (Proton compares it).
pub fn write(client_dir: &Path) -> io::Result<()> {
    let dir = client_dir.join("legacycompat");
    std::fs::create_dir_all(&dir)?;
    for arch in [Arch::X86, Arch::X64] {
        let want = image(arch);
        let dest = dir.join(arch.file_name());
        if std::fs::read(&dest).is_ok_and(|have| have == want) {
            continue;
        }
        let tmp = dir.join(format!(".{}.ossteam", arch.file_name()));
        std::fs::write(&tmp, &want)
            .and_then(|()| std::fs::rename(&tmp, &dest))
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
    }

    fn u32_at(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }

    /// A small PE reader: (rva, file offset, size) of each section.
    fn sections(b: &[u8]) -> Vec<(u32, u32, u32)> {
        let pe = u32_at(b, 0x3C) as usize;
        let count = u16_at(b, pe + 6) as usize;
        let first = pe + 24 + u16_at(b, pe + 20) as usize;
        (0..count)
            .map(|i| {
                let h = first + 40 * i;
                (u32_at(b, h + 12), u32_at(b, h + 20), u32_at(b, h + 8))
            })
            .collect()
    }

    fn at_rva(b: &[u8], rva: u32) -> usize {
        let (base, off, _) = sections(b)
            .into_iter()
            .rfind(|&(r, _, _)| r <= rva)
            .expect("a section holds the rva");
        (off + rva - base) as usize
    }

    fn c_str(b: &[u8], o: usize) -> &str {
        let end = b[o..].iter().position(|&c| c == 0).unwrap();
        std::str::from_utf8(&b[o..o + end]).unwrap()
    }

    #[test]
    fn the_names_are_sorted_for_the_loaders_binary_search() {
        assert!(
            EXPORTS
                .windows(2)
                .all(|w| w[0].as_bytes() < w[1].as_bytes())
        );
    }

    #[test]
    fn each_export_names_its_own_function_that_returns_zero() {
        for arch in [Arch::X86, Arch::X64] {
            let b = image(arch);
            assert_eq!(&b[..2], b"MZ");
            let pe = u32_at(&b, 0x3C) as usize;
            assert_eq!(&b[pe..pe + 4], b"PE\0\0");
            let machine = u16_at(&b, pe + 4);
            let magic = u16_at(&b, pe + 24);
            match arch {
                Arch::X86 => assert_eq!((machine, magic), (0x014C, 0x10B)),
                Arch::X64 => assert_eq!((machine, magic), (0x8664, 0x20B)),
            }
            // DLL, executable image.
            assert_eq!(u16_at(&b, pe + 22) & 0x2002, 0x2002);
            let dirs = pe + 24 + if arch == Arch::X64 { 112 } else { 96 };
            let export_rva = u32_at(&b, dirs);
            let e = at_rva(&b, export_rva);
            assert_eq!(c_str(&b, at_rva(&b, u32_at(&b, e + 12))), arch.file_name());
            assert_eq!(u32_at(&b, e + 16), 1);
            let n = u32_at(&b, e + 24) as usize;
            assert_eq!(n, EXPORTS.len());
            let functions = at_rva(&b, u32_at(&b, e + 28));
            let names = at_rva(&b, u32_at(&b, e + 32));
            let ordinals = at_rva(&b, u32_at(&b, e + 36));
            let mut seen = Vec::new();
            for (i, want) in EXPORTS.iter().enumerate() {
                assert_eq!(c_str(&b, at_rva(&b, u32_at(&b, names + 4 * i))), *want);
                let ordinal = u16_at(&b, ordinals + 2 * i) as usize;
                let f = u32_at(&b, functions + 4 * ordinal);
                let code = at_rva(&b, f);
                assert_eq!(&b[code..code + 3], &[0x31, 0xC0, 0xC3]);
                seen.push(f);
            }
            // Each function has room for the 12-byte jump.
            seen.sort();
            assert!(seen.windows(2).all(|w| w[1] - w[0] >= 12));
            // The entry point returns TRUE and is not an export.
            let entry = u32_at(&b, pe + 24 + 16);
            assert!(!seen.contains(&entry));
            assert_eq!(&b[at_rva(&b, entry)..][..5], &[0xB8, 1, 0, 0, 0]);
            // Sections lie inside the file and SizeOfImage covers them.
            let image_size = u32_at(&b, pe + 24 + 56);
            for (rva, off, size) in sections(&b) {
                assert!((off + size) as usize <= b.len());
                assert!(rva + size <= image_size);
            }
        }
    }

    #[test]
    fn writing_twice_leaves_the_files_alone() {
        let dir = std::env::temp_dir().join(format!("ossteam-client-dll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(&dir).unwrap();
        let file = dir.join("legacycompat").join("steamclient.dll");
        assert_eq!(std::fs::read(&file).unwrap(), image(Arch::X86));
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before
        );
        assert_eq!(
            std::fs::read(dir.join("legacycompat").join("steamclient64.dll")).unwrap(),
            image(Arch::X64)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
