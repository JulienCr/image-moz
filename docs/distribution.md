# Distribution

Image Moz se distribue sous la forme d'un installeur Inno Setup 6 qui installe
l'application pour tous les utilisateurs de la machine.

## Prérequis

- Rust (toolchain MSVC) et `cargo`.
- Inno Setup 6 (`C:\Program Files (x86)\Inno Setup 6\ISCC.exe`).
- ImageMagick 7 (`magick` dans le PATH), uniquement pour régénérer l'icône.

## Régénérer l'icône

La source est `assets/icon.svg`. Pour reconstruire `assets/icon.ico` (tailles
16, 20, 24, 32, 40, 48, 64 et 256) et `assets/icon-256.png` :

```powershell
./assets/build-icon.ps1
```

L'icône est intégrée à l'exe par `build.rs` (crate `winresource`), avec les
informations de version (« Image Moz »). Après un changement de l'icône,
relancer `cargo build --release`.

## Construire l'installeur

```powershell
./installer/build.ps1
```

Le script lance `cargo build --release`, puis compile `installer/image-moz.iss`
avec ISCC. La version est lue dans `Cargo.toml`. Il s'arrête à la première
erreur. Le résultat est `installer/Output/image-moz-setup-<version>.exe`
(dossier ignoré par git).

## Ce que l'installeur écrit

- L'exe dans `C:\Program Files\Image Moz` (élévation UAC demandée), un
  raccourci dans le menu Démarrer et un désinstalleur.
- Le menu contextuel « Ouvrir dans Image Moz » (avec l'icône de l'app), sous
  `HKLM\Software\Classes` :
  - `Directory\shell\ImageMoz` : clic droit sur un dossier (argument `%1`) ;
  - `Directory\Background\shell\ImageMoz` : clic droit sur le fond d'un
    dossier ouvert (argument `%V`) ;
  - `SystemFileAssociations\.<ext>\shell\ImageMoz` pour `png`, `jpg`, `jpeg`,
    `webp`, `bmp`, `gif`, `tif`, `tiff`, `exr` et `hdr` (argument `%1`).

Sous Windows 11, ces entrées apparaissent sous « Afficher plus d'options ».
L'application par défaut des images n'est pas modifiée. Si les formats
supportés changent (`src/formats.rs`), mettre à jour la liste dans
`installer/image-moz.iss`.

## Désinstaller

Via « Applications installées » de Windows, ou le désinstalleur du dossier
d'installation. Il retire l'exe, le raccourci et les clés de registre ci-dessus
(uniquement celles créées par l'installeur). La configuration
(`%APPDATA%\image-moz`) et le cache de miniatures (`%LOCALAPPDATA%\image-moz`)
de chaque utilisateur sont conservés.
