# Support 3D — suites à donner

Points relevés par les relectures du support 3D (§15 de la spec) et laissés
de côté à la fusion (`9bc526b`, 2026-11-12). Aucun ne casse l'usage courant.
Classés par priorité dans chaque section.

## Robustesse et performance

- **Mémoire de pointe avec plusieurs gros modèles.** Chaque worker de
  vignettes (nombre de cœurs − 1) charge un modèle entier en parallèle :
  buffers glTF, textures décodées, `Scene`, copie GPU. Un dossier de scans de
  plusieurs centaines de Mo pourrait saturer la mémoire. **Non mesuré** : à
  vérifier avec un vrai dossier de scans. Piste si c'est confirmé : un
  sémaphore d'environ 2 autour du chargement et du rendu dans
  `formats::decode`.
- **Création du GPU des vignettes sans garde-fou.** En test, créer plusieurs
  devices wgpu en même temps bloquait parfois sous DX12. Un mutex dans
  `Offscreen::new` a contourné le problème, mais la cause reste inconnue. Si
  un blocage survient dans `Offscreen::shared()`, tous les workers qui
  prennent un modèle restent bloqués. Piste : délai maximal, ou au moins un
  log.
- **Course au F5.** Un rendu de vignette en cours au moment du F5 peut
  réécrire l'ancienne image ; un second F5 corrige. Piste : étiqueter chaque
  job avec une génération incrémentée par `forget_all`, et ignorer les
  résultats périmés (une quinzaine de lignes).
- **Textures volumineuses.** Pas de mipmaps : crénelage sur les vignettes et
  pic de mémoire vidéo. Les modes de répétition des textures sont ignorés
  (toujours `Repeat`).
- **Redimensionnement de l'aperçu.** La cible MSAA est reconstruite à chaque
  frame pendant qu'on redimensionne la fenêtre. Piste : arrondir la taille
  par paliers.
- **`.blend` malformés.** Un bloc `REND` de longueur aberrante fait lire et
  décompresser tout le fichier (la mémoire reste bornée). Les erreurs de
  décompression remontent en `Io` au lieu de `Format`. Les octets de taille
  de pointeur et d'endianness de l'en-tête v5 ne sont pas validés.
- **3MF.**
  - Les identifiants d'objets s'écrasent entre plusieurs fichiers `.model`
    d'une même archive (projets Bambu Studio et Orca).
  - Les couleurs ne sont pas lues : le crate `threemf` 0.8.1 ne remplit pas
    `basematerials`.
  - L'unité est ignorée, ce qui ne gêne pas grâce au cadrage automatique.

## Fidélité du rendu

- **glTF chargé en bloc.** `gltf::import` échoue sur tout le modèle si une
  seule texture externe manque, ou si une image est en WebP ou KTX2. Un OBJ,
  lui, retombe sur la couleur par défaut. Piste : `Gltf::open` puis
  `import_buffers`, et décoder chaque image séparément.
- **Liseré clair sur les bords du modèle dans l'aperçu.** La texture est
  prémultipliée en lumière linéaire, alors qu'egui attend une
  prémultiplication en gamma. Les vignettes et `Ctrl+C` sont déjà corrigés
  par `unpremultiply_edges`.
- **Jeu d'UV.** Seul le premier jeu d'UV est lu ; le `texCoord` du matériau
  est ignoré.
- **Caméra du fichier.** L'inclinaison latérale (roll) est perdue, et
  l'aperçu s'ouvre de niveau. Une caméra dont l'axe de visée est parallèle à
  l'axe vertical (nœud cisaillé) n'est pas rejetée.
- **Adaptateur OpenGL seul.** `RenderTarget` déclare toujours la vue
  non-sRGB, nécessaire seulement pour l'aperçu. Les vignettes échoueraient
  sur un adaptateur sans `VIEW_FORMATS`, ce qui n'arrive pas sous Windows
  (DX12 ou Vulkan).

## Interface

- **« N images »** dans la barre d'état et dans les en-têtes de section :
  les modèles y sont comptés comme des images.
- **Orbite au pavé tactile impossible.** L'orbite n'utilise que le bouton du
  milieu. Piste : Alt + glisser au clic gauche, comme l'option « émuler 3
  boutons » de Blender.
- **`Ctrl+C` sur un modèle avant le premier rendu.** Le toast « Copie en
  cours… » s'affiche, puis l'app copie un rendu de la vue de départ au lieu
  de la vue courante.
- **Nettoyage.** Le bras « Chargement… » de `ModelView::paint` n'est jamais
  atteint, et `ModelView::error()` et `camera()` ne servent qu'aux tests. La
  bascule d'axe et `FULL_UV` sont dupliqués : une méthode `UpAxis::flipped`
  supprimerait le doublon.

## Vérifications et tests manquants

- **`.blend` compressé avec vignette.** Le chemin zstd n'est prouvé que sur
  des données synthétiques. À vérifier une fois : sauver un fichier avec
  Blender 5.3, « Compress » coché et aperçu activé, puis regarder sa tuile.
- **Limites GPU de la fenêtre.** Aucun test automatique, et aucun essai réel
  avec un maillage de plus de 256 Mo dans l'aperçu.
- **Galerie.**
  - Aucun test ne vérifie qu'une tuile « pas d'aperçu » n'est plus
    redemandée.
  - Aucun test ne vérifie que le nombre de triangles arrive jusqu'à
    `Entry`.
- **Géométrie.**
  - Le retournement du V des UV d'un OBJ n'est pas testé, ni la rotation
    d'un 3MF.
  - Le sens et l'amplitude de l'orbite et du déplacement ne sont pas
    testés, ni l'orientation du repère.
  - Les bornes du champ de vue ne sont pas testées.
- **Rendu.**
  - Les seuils de couleur sont lâches : aucune demi-teinte, aucun test
    d'ordre de profondeur.
  - Le test de concurrence ne vérifie qu'un pixel par rendu, et le test
    des lignes non alignées deux pixels.
- **`.blend`.**
  - L'arrêt au premier bloc inconnu n'est pas épinglé par un test (le
    `GLOB` du test est petit).
  - Les dimensions hors bornes et les longueurs aberrantes ne sont pas
    testées.
