// Ressourcen des Installers: Programmsymbol, Manifest und Versionsinfo.
//
// Das Manifest ist nicht Kosmetik. Eine Datei, die "Setup" im Namen traegt und
// kein Manifest hat, wird von Windows fuer einen alten Installer gehalten und
// mit Administratorrechten gestartet – genau das darf nicht passieren, denn
// Aura installiert pro Benutzer, damit der eingebaute Updater die Dateien
// spaeter selbst tauschen kann. `asInvoker` sagt Windows: so lassen.
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../assets/aura.ico");

    let out = env::var("OUT_DIR").unwrap();
    let out = Path::new(&out);
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (a, b, c) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );

    let manifest = out.join("aura-setup.manifest");
    fs::write(
        &manifest,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="AuraBrowser.Setup" version="{a}.{b}.{c}.0" processorArchitecture="*"/>
  <description>Aura Browser Setup</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
      <supportedOS Id="{{1f676c76-80e1-4239-95bb-83d0f6d0da78}}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#
        ),
    )
    .unwrap();

    let ico = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("assets")
        .join("aura.ico");
    let ico = ico.canonicalize().unwrap_or(ico);
    let slashes = |p: &Path| p.to_string_lossy().replace('\\', "/").replace("//?/", "");

    let rc = format!(
        r#"1 ICON "{ico}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Edgar Kessler"
      VALUE "FileDescription", "Aura Browser Setup"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "aura-setup"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "AuraBrowserSetup.exe"
      VALUE "ProductName", "Aura Browser"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        ico = slashes(&ico),
        manifest = slashes(&manifest),
    );
    let rc_path = out.join("aura-setup.rc");
    fs::write(&rc_path, rc).unwrap();
    embed_resource::compile(&rc_path, embed_resource::NONE);

    // MinGW haengt jedem Programm ein eigenes Standard-Manifest an
    // (default-manifest.o aus der CRT). Zusammen mit unserem waeren das zwei,
    // und welches Windows dann liest, ist Glueckssache. gcc sucht die Datei
    // ueber seine Praefix-Pfade – ein leeres Objekt gleichen Namens, das per
    // -B vorne eingereiht wird, gewinnt und traegt nichts bei.
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        let machine: u16 = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86") => 0x014c,
            Ok("aarch64") => 0xaa64,
            _ => 0x8664,
        };
        let dir = out.join("no-default-manifest");
        fs::create_dir_all(&dir).unwrap();
        // Leerer COFF-Kopf: Maschine, 0 Sektionen, Zeitstempel, Symboltabelle,
        // 0 Symbole, keine optionale Kopfzeile, keine Merkmale.
        let mut obj = Vec::with_capacity(20);
        obj.extend_from_slice(&machine.to_le_bytes());
        obj.extend_from_slice(&0u16.to_le_bytes());
        obj.extend_from_slice(&0u32.to_le_bytes());
        obj.extend_from_slice(&0u32.to_le_bytes());
        obj.extend_from_slice(&0u32.to_le_bytes());
        obj.extend_from_slice(&0u16.to_le_bytes());
        obj.extend_from_slice(&0u16.to_le_bytes());
        fs::write(dir.join("default-manifest.o"), obj).unwrap();
        println!("cargo:rustc-link-arg-bins=-B{}/", slashes(&dir));
    }
}
