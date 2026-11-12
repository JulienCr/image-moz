; Version is passed by build.ps1 (/DAppVersion=...) from Cargo.toml.
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#define ExeName "image-moz.exe"
#define MenuLabel "Ouvrir dans Image Moz"

[Setup]
AppId={{B7C4E1A2-5D3F-4A8E-9C61-2F0D7E8A41B3}
AppName=Image Moz
AppVersion={#AppVersion}
AppPublisher=Image Moz
DefaultDirName={autopf}\Image Moz
DefaultGroupName=Image Moz
PrivilegesRequired=admin
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#ExeName}
UninstallDisplayName=Image Moz
OutputDir=Output
OutputBaseFilename=image-moz-setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Languages]
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Files]
Source: "..\target\release\{#ExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Image Moz"; Filename: "{app}\{#ExeName}"

[Registry]
; Only keys owned by Image Moz are created, each removed on uninstall.
; Default app and ProgIDs for images are never touched.
Root: HKLM; Subkey: "Software\Classes\Directory\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\Directory\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\Directory\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\Directory\Background\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\Directory\Background\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\Directory\Background\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%V"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.png\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.png\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.png\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.jpg\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.jpg\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.jpg\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.jpeg\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.jpeg\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.jpeg\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.webp\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.webp\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.webp\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.bmp\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.bmp\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.bmp\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.gif\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.gif\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.gif\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.tif\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.tif\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.tif\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.tiff\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.tiff\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.tiff\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.exr\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.exr\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.exr\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.hdr\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.hdr\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.hdr\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.glb\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.glb\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.glb\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.gltf\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.gltf\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.gltf\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.obj\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.obj\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.obj\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.stl\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.stl\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.stl\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.3mf\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.3mf\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.3mf\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.blend\shell\ImageMoz"; ValueType: string; ValueData: "{#MenuLabel}"; Flags: uninsdeletekey
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.blend\shell\ImageMoz"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\{#ExeName}"",0"
Root: HKLM; Subkey: "Software\Classes\SystemFileAssociations\.blend\shell\ImageMoz\command"; ValueType: string; ValueData: """{app}\{#ExeName}"" ""%1"""
