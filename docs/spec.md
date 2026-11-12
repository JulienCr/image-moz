# image-moz — spécification

Visionneuse d'images de bureau pour Windows : elle affiche le contenu d'un
dossier en grille et se met à jour dès qu'un fichier y apparaît, change ou
disparaît. Cas d'usage d'origine : suivre en direct un dossier où un outil
(Blender, un script, un agent) dépose des rendus, par exemple
`C:\Users\julie\AppData\Local\Temp\claude\…\scratchpad`.

Décisions issues d'un grilling en trois rounds (2026-10-29).

## 1. Socle

| Sujet | Décision |
|---|---|
| Langage / GUI | Rust, `egui` / `eframe` |
| Cible | exe Windows natif, toolchain `stable-x86_64-pc-windows-msvc` (cargo 1.92) |
| Emplacement | `D:\dev\image-moz`, build et run avec `cargo.exe` côté Windows (pas depuis WSL) |
| Git | dépôt local, pas de remote pour l'instant |
| Langue | code, identifiants et commentaires en anglais ; libellés d'interface en français |

## 2. Dossier source

- Trois façons de choisir le dossier : argument en ligne de commande
  (`image-moz.exe <dossier>`), glisser-déposer d'un dossier sur la fenêtre,
  bouton « Ouvrir » (sélecteur natif). Chacune ouvre un onglet (§13).
- La session (fenêtres et onglets) est mémorisée et restaurée au lancement (§13).
- **Sous-dossiers** : un toggle « Récursif » dans la barre d'outils, désactivé
  par défaut.
  - En mode récursif, l'affichage est **découpé en sections**, une par dossier,
    chacune avec un en-tête (chemin relatif).
  - Ordre des sections : le dossier racine d'abord, puis les sous-dossiers
    selon le tri courant (en tri par date, un sous-dossier remonte quand il
    reçoit une image).
  - Une section se replie au clic sur son en-tête ; l'état replié est
    mémorisé par dossier.

## 3. Formats

PNG, JPEG, WebP, BMP, GIF (première frame seulement), TIFF, EXR, HDR.
Modèles 3D : GLB, glTF, OBJ, STL, 3MF, `.blend` (§15).

- EXR / HDR : tone-mapping **ACES** fixe, exposition 0, sortie sRGB. Aucun
  réglage dans l'interface.
- Un fichier illisible ou partiellement écrit ne fait pas planter l'app : la
  tuile affiche un état d'erreur et est retentée au prochain événement de
  modification.

## 4. Surveillance du dossier

- Événements gérés : création, modification (fichier réécrit sous le même
  nom), suppression, renommage.
- **Debounce** des écritures : on n'affiche un fichier qu'une fois son écriture
  stabilisée, pour ne pas décoder un PNG à moitié écrit.
- Modification → la miniature est régénérée (le cache est invalidé).
- Suppression → la tuile disparaît.
- Arrivée d'une nouvelle image : **surlignage bref** de la tuile. La position
  de scroll ne bouge pas, sauf si la vue était déjà tout en haut (elle y reste
  et la nouveauté est visible).
- `F5` force un rescan complet.
- Les dossiers WSL (`\\wsl.localhost\`, `\\wsl$\`, ou un lecteur réseau qui y
  pointe) n'émettent aucune notification Windows : ils sont sondés toutes les 2 s.

## 5. Tri

Plus récent d'abord (date de modification) par défaut. Menu pour changer :
nom, date (dans les deux sens). Le tri s'applique à l'intérieur de chaque
section.

## 6. Layouts

Trois modes, changés par la barre d'outils ou les touches `1` / `2` / `3` :

1. **Grille uniforme** — tuiles carrées, image ajustée dedans.
2. **Justified (Flickr)** — rangées de hauteur égale, largeur proportionnelle
   au ratio, chaque rangée remplit la largeur de la fenêtre.
3. **Bento / Mosaic** — mosaïque de tuiles de tailles variées.
   - Motif : une tuile 2×2, puis des tuiles 2×1 ou 1×2 choisies selon le ratio
     de l'image (paysage → 2×1, portrait → 1×2), motif répété.
   - **Ce qui est mis en avant** se choisit dans un menu : les plus récentes,
     les épinglées, ou les plus grandes résolutions.
   - Épingler / désépingler l'image sélectionnée : touche `P`. Les épingles sont
     mémorisées **par dossier** dans la config de l'app ; une épingle dont le
     fichier n'existe plus est ignorée.
   - En mode sections, le motif repart à chaque en-tête : chaque section a sa
     propre tuile 2×2.

### Taille

- Un seul réglage : une **taille cible en pixels**, que chaque layout interprète
  (côté de tuile en grille, hauteur de rangée en justified, taille de cellule
  de base en bento).
- Slider dans la barre d'outils ; raccourcis `Ctrl+molette`, `Ctrl+=` /
  `Ctrl+-` (et `Ctrl++`).
- Valeur mémorisée.

### Fenêtre

Redimensionnable ; le layout se recalcule à chaque changement de largeur. Seules
les tuiles visibles sont dessinées et chargées (virtualisation), le dossier
pouvant contenir des milliers d'images.

## 7. Infos affichées

Rien sur les tuiles. Au survol : nom, dimensions, poids du fichier (et chemin
relatif en mode récursif). Une barre d'état en bas affiche la même chose pour
l'image survolée ou sélectionnée, plus le nombre d'images et la sélection.

## 8. Sélection et clavier

| Entrée | Action |
|---|---|
| clic | sélectionne une image |
| `Ctrl+clic` / `Shift+clic` | ajoute / étend la sélection |
| flèches | déplacent la sélection (dans la géométrie du layout courant) |
| double-clic | ouvre l'image dans l'application Windows par défaut |
| clic droit | menu : « Afficher dans l'Explorateur » (`explorer /select,`) |
| `Ctrl+C` | copie **les fichiers** sélectionnés (format fichier, collable dans l'Explorateur) **et** le **bitmap** de l'image focus (collable dans Discord, Claude, etc.), sur le même presse-papier |
| `Ctrl+Alt+C` | copie le ou les chemins absolus, un par ligne |
| `Espace` | aperçu plein cadre |
| `P` | épingle / désépingle |
| `1` `2` `3` | grille / justified / bento |
| `F5` | rescan |

L'app est en **lecture seule** : aucune suppression, aucun renommage.

## 9. Aperçu plein cadre

- `Espace` ouvre l'image focus en plein cadre dans la fenêtre, décodée depuis
  l'original (pas la miniature).
- Flèches gauche / droite : image précédente / suivante ; `Échap` ou `Espace`
  ferme.
- Ajusté à la fenêtre à l'ouverture ; molette = zoom centré sur le curseur ;
  glisser = déplacement ; `0` = revenir à l'ajustement.
- Ctrl+C / Ctrl+Alt+C fonctionnent aussi dans l'aperçu.

## 10. Cache des miniatures

- Miniatures de **512 px** sur le grand côté, décodées dans des threads de
  fond, jamais sur le thread UI.
- Cache disque sous `%LOCALAPPDATA%\image-moz\thumbs`. Clé : chemin absolu +
  date de modification + taille du fichier.
- Plafond **1 Go** ; au démarrage, purge des entrées les moins récemment
  utilisées jusqu'à repasser sous le plafond.

## 11. Configuration persistée

Fichier de config (`%APPDATA%\image-moz\`) : session (fenêtres, onglets,
onglet actif, position et taille des fenêtres), layout, taille, règle Bento, et
par dossier : épingles, sections repliées, toggle récursif et tri.

L'ancien champ `last_folder` est lu une fois (migration) : il devient l'unique
onglet de la session si aucune session n'est enregistrée.

## 12. Hors périmètre (v1)

- Actions destructives (suppression, renommage, déplacement).
- Réglage d'exposition HDR.
- GIF animés (première frame seulement).
- Remote git.
- Entrée dans le menu contextuel principal de Windows 11 (`IExplorerCommand`
  + paquet MSIX signé) : seule l'entrée classique est fournie.

## 13. Onglets et fenêtres

Décisions issues d'un grilling (2026-11-07).

### Onglets

- Chaque onglet affiche un dossier, avec sa propre bibliothèque, son watcher,
  son scroll, sa sélection et son aperçu plein cadre.
- Barre d'onglets au-dessus de la barre d'outils. Libellé : nom du dossier,
  chemin complet au survol. Fermeture par `×` ou clic milieu.
- `Ctrl+T` : sélecteur de dossier, résultat dans un nouvel onglet. `Ctrl+W` :
  ferme l'onglet actif. `Ctrl+Tab` / `Ctrl+Shift+Tab` : onglet suivant /
  précédent.
- Bouton « Ouvrir », glisser-déposer d'un dossier et ligne de commande ouvrent
  **un nouvel onglet**.
- Un dossier déjà ouvert (comparaison via la clé de dossier, insensible à la
  casse) **active l'onglet existant**, quelle que soit sa fenêtre, au lieu d'en
  créer un second.
- Ouvrir un **fichier image** (ligne de commande, menu contextuel) ouvre son
  dossier parent et sélectionne cette image, en la faisant défiler à l'écran.
- Fermer le dernier onglet de la dernière fenêtre laisse la fenêtre ouverte, vide.

### Réglages

- **Par dossier** (mémorisés dans la config) : toggle « Récursif » et tri,
  en plus des épingles et des sections repliées. Un dossier jamais vu prend
  les valeurs par défaut (non récursif, date ↓).
- **Globaux** : layout, taille, règle Bento.

### Onglets en arrière-plan

- Ils restent surveillés : le watcher tourne, les changements sont appliqués à
  leur bibliothèque.
- L'onglet affiche un badge `(n)` du nombre d'images arrivées depuis sa
  dernière activation ; le compteur repart à zéro quand on l'active.
- Les miniatures ne sont demandées que pour l'onglet visible de chaque fenêtre.
  Un seul pool de décodage et un seul cache disque servent tous les onglets.

### Fenêtres multiples

- Glisser un onglet hors de toute barre d'onglets et le relâcher le **détache**
  dans une nouvelle fenêtre, ouverte sous le curseur.
- Le relâcher sur la barre d'onglets d'une autre fenêtre l'y **déplace**, à la
  position du curseur ; c'est ainsi qu'on fusionne deux fenêtres. Sur sa propre
  barre, le glisser **réordonne** les onglets.
- Une fenêtre dont le dernier onglet part se ferme. L'app se termine quand la
  dernière fenêtre est fermée.
- Toutes les fenêtres vivent dans le même processus.

### Instance unique

- Un seul processus Image Moz par session utilisateur. Un second lancement
  transmet son argument au processus existant puis se termine.
- Le chemin reçu s'ouvre dans un nouvel onglet de la **dernière fenêtre
  active**, qui passe au premier plan.

### Session

- Au lancement sans argument, toutes les fenêtres et tous les onglets de la
  dernière session sont restaurés, avec l'onglet actif, la position et la
  taille de chaque fenêtre. Un dossier disparu entre-temps est ignoré.
- Au lancement avec un argument, la session est restaurée puis le chemin
  s'ouvre comme décrit plus haut.

## 14. Distribution

- **Nom affiché** : « Image Moz » (titre de fenêtre, menu Démarrer, installeur,
  menu contextuel). L'exécutable reste `image-moz.exe`.
- **Icône** : mosaïque Bento (une grande tuile, deux petites), lisible à 16 px.
  Source SVG versionnée, `.ico` multi-tailles (16, 20, 24, 32, 40, 48, 64, 256)
  intégré à l'exe et utilisé comme icône de fenêtre.
- **Installeur** : Inno Setup 6, installation **pour toute la machine**
  (`Program Files\Image Moz`, élévation UAC), raccourci dans le menu Démarrer,
  désinstalleur. La désinstallation retire l'exe et les clés de registre, et
  laisse la config et le cache de miniatures de chaque utilisateur.
- **Menu contextuel** « Ouvrir dans Image Moz », entrée classique sous
  `HKLM\Software\Classes` (visible sous « Afficher plus d'options » sur
  Windows 11), avec l'icône de l'app :
  - clic droit sur un dossier (`Directory\shell`) ;
  - clic droit sur le fond d'un dossier ouvert (`Directory\Background\shell`,
    argument `%V`) ;
  - clic droit sur une image d'un format supporté
    (`SystemFileAssociations\<ext>\shell`, une clé par extension) : sans
    toucher à l'application par défaut des images.

## 15. Modèles 3D

Décisions issues d'un grilling en deux rounds (2026-11-11).

### Formats et sources d'image

| Format | Source de l'image | Orbite |
|---|---|---|
| GLB, glTF | rendu GPU | oui |
| OBJ (+ `.mtl`) | rendu GPU | oui |
| STL, 3MF | rendu GPU | oui |
| `.blend` | vignette embarquée dans le fichier, lue en Rust | non |

- `.blend` : pas de dépendance à Blender. Sans vignette embarquée, la tuile
  affiche « pas d'aperçu ». L'aperçu plein cadre agrandit la vignette.
- Version de Blender de référence : 5.3 (Steam).

### Rendu

- **GPU via wgpu.** Les vignettes ont leur **propre device**, indépendant de
  la fenêtre ; l'orbite dessine avec le device d'eframe.
- Sans adaptateur matériel (bureau à distance, VM), repli sur l'adaptateur
  logiciel (WARP).
- Apparence : couleur de base + texture principale des matériaux. Ni normal
  maps, ni métal-rugosité, ni transparence.
- Vignette à fond transparent ; un modèle animé ou riggé est rendu dans sa
  pose de repos.
- Caméra : la première caméra de la scène du fichier si elle existe, sinon
  vue 3/4 auto-cadrée sur la boîte englobante.
- Axe vertical sans caméra : glTF et OBJ en Y-up, STL et 3MF en Z-up.

### Grille et barre d'état

- Badge de format (`GLB`, `OBJ`, `STL`, `3MF`, `BLEND`…) sur la tuile.
- La barre d'état affiche le nombre de triangles à la place des dimensions
  (rien pour un `.blend`).

### Aperçu plein cadre d'un modèle

| Entrée | Action |
|---|---|
| glisser bouton du milieu | orbite |
| `Maj` + glisser bouton du milieu | déplacement |
| molette | rapprocher / éloigner |
| `0` | retour à la caméra de départ |
| `U` | bascule l'axe vertical Y / Z, pour la session |

Flèches, `Échap`, `Espace`, `Ctrl+C` (bitmap de la vue courante) et
`Ctrl+Alt+C` gardent leur rôle du §9.

### Cache

La clé reste celle du §10 (fichier principal seul). Une annexe modifiée seule
(`.bin`, `.mtl`, texture) laisse la vignette périmée jusqu'à `F5` ou la
réécriture du modèle : limite assumée.

### Livraison

Une seule branche pour l'ensemble.

## Annexe — briques envisagées (à valider à l'implémentation)

Indicatif, non décidé : `eframe`/`egui`, `image` (features formats + `exr`),
`notify` + `notify-debouncer-full`, `rfd` (sélecteur de dossier), `arboard`
(bitmap) et `clipboard-win` ou `windows` (format fichier `CF_HDROP`, pour poser
les deux formats sur le même presse-papier), `serde` + `serde_json` pour la
config, `directories` pour les chemins Windows.
