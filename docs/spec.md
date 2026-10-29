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
  bouton « Ouvrir » (sélecteur natif).
- Le dernier dossier ouvert est mémorisé et rouvert au lancement sans argument.
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

Fichier de config (`%APPDATA%\image-moz\`) : dernier dossier, layout, taille,
tri, toggle récursif, règle Bento, et par dossier : épingles et sections
repliées.

## 12. Hors périmètre (v1)

- Actions destructives (suppression, renommage, déplacement).
- Réglage d'exposition HDR.
- GIF animés (première frame seulement).
- Remote git / distribution / installeur.

## Annexe — briques envisagées (à valider à l'implémentation)

Indicatif, non décidé : `eframe`/`egui`, `image` (features formats + `exr`),
`notify` + `notify-debouncer-full`, `rfd` (sélecteur de dossier), `arboard`
(bitmap) et `clipboard-win` ou `windows` (format fichier `CF_HDROP`, pour poser
les deux formats sur le même presse-papier), `serde` + `serde_json` pour la
config, `directories` pour les chemins Windows.
