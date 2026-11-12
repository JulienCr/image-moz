# Refonte de l'interface : « table lumineuse en direct »

Spécification versionnée de la refonte GUI d'image-moz (egui 0.35). Elle remplace les maquettes (canvas de design, non versionné) : toutes les valeurs utiles y sont reprises. Les noms de code restent en anglais ; les libellés visibles restent en français.

Maquettes sources : Spec (décisions, palette, mapping), États (tuiles, onglets, en-têtes, aperçu 3D), Clair (thème clair par défaut, Justifié, sections récursives), Principal (thème sombre, Bento), Aperçu (plein cadre), Vide (fenêtre vide, filtre vide).

## 1. Direction

Le chrome se retire dans un graphite neutre et les images occupent tout le reste. Deux couleurs seulement portent du sens : l'ambre pour ce qui arrive (surveillance, nouveaux fichiers, badges) et le bleu pour ce que l'utilisateur a choisi (sélection, focus, contrôle actif). L'outil reste ouvert à côté de Blender ou d'un agent : il se fait oublier jusqu'à l'arrivée d'un rendu, puis le signale sans bruit. Le thème clair est le défaut ; le sombre est au choix (Clair / Sombre / Système).

### Les 8 décisions

1. **Bandes grises en Bento.** Remplissage (cover) centré en Grille et en Bento ; Justifié reste exact par construction. Pas de bascule au survol (la mosaïque bougerait sous le curseur) : l'infobulle montre la miniature entière. Option ultérieure : réglage « Ajuster / Remplir » par dossier.
2. **Chrome clair contre tuiles sombres.** Un seul jeu de surfaces par thème ; le placeholder découle du fond de galerie (fin du gris 44 fixe). Clair par défaut ; sombre au choix ou suivi de l'OS.
3. **Onglets et barre d'outils confondus.** Barre d'onglets plus sombre (36 px) ; l'onglet actif prend la couleur de la barre d'outils et s'y raccorde, coins hauts 6 px. Le × n'apparaît que sur l'onglet actif ou survolé.
4. **Barre d'outils plate.** Quatre groupes séparés : Ouvrir · Récursif/À plat (puces) · disposition (segmenté à icônes) + tri + règle Bento · à droite, taille et filtre. Le « 200 px » quitte la barre et s'affiche dans l'infobulle du slider pendant le glisser.
5. **Barre d'état illisible.** 30 px, 12,5 px, alignée à gauche : nom en fort, dimensions et poids en mono, chemin relatif en faible. Compteurs à droite, sélection en bleu. Ajout d'un témoin « En direct » (état du watcher, déjà connu de l'app).
6. **Titre UNC à rallonge.** « dossier — Image Moz » ; le chemin complet reste au survol de l'onglet. Barre de titre native gardée, accordée au thème avec `ViewportCommand::SetTheme`. Une barre custom ne vaut pas son coût (snap layouts Windows 11, bords, double-clic).
7. **Pas d'identité.** L'icône mosaïque ambre/bleu/blanc fournit la palette ; Inter pour l'interface, JetBrains Mono pour chiffres et chemins. Rien d'autre : l'identité, ce sont les images.
8. **États non dessinés.** Punaise sur les épinglées (toutes dispositions), pastille ambre « (n) » sur les onglets, « +n nouvelles » sur les en-têtes de section, chrome complet pour l'aperçu.

## 2. Palette

Constructeur : `Color32::from_rgb(r, g, b)`. Le thème clair (`light_visuals()`) reprend le squelette de `dark_visuals()` (section 5) avec la colonne « Clair », base `egui::Visuals::light()`.

| Rôle | Sombre (RGB) | Clair (RGB) |
|---|---|---|
| Fond de galerie | 17, 17, 19 | 228, 228, 231 |
| Barre de titre / onglets | 20, 20, 22 | 235, 235, 238 |
| Barre d'outils, onglet actif | 30, 30, 34 | 250, 250, 251 |
| Barre d'état, en-têtes | 24, 24, 27 | 243, 243, 245 |
| Popups, infobulles | 38, 38, 43 | 255, 255, 255 |
| Champ texte, rail segmenté | 20, 20, 22 | 235, 235, 238 |
| Bouton plein (Ouvrir) | 42, 42, 48 | 255, 255, 255 + bord |
| Bordure / séparateur | 42, 42, 48 | 220, 220, 225 |
| Texte | 232, 232, 236 | 24, 24, 27 |
| Texte faible | 150, 150, 160 | 95, 95, 107 |
| Accent : sélection, focus | 77, 148, 255 | 31, 111, 235 |
| Contrôle actif (fond) | 34, 50, 77 | 221, 233, 253 |
| Contrôle actif (texte) | 184, 210, 255 | 23, 78, 166 |
| Nouvelle arrivée, en direct | 255, 184, 48 | 224, 150, 0 |
| Erreur | 255, 138, 92 | 212, 68, 47 |
| Placeholder de tuile | 28, 28, 32 | 212, 212, 216 |
| Fond d'aperçu | 18, 18, 18 (les deux thèmes) | 18, 18, 18 |
| Badge format, punaise | `from_black_alpha(160)` + blanc | idem |

Contrastes vérifiés : texte faible 5,6:1 (sombre, sur barre d'outils) et 6:1 (clair) ; accent clair #1F6FEB 4,6:1 sur blanc. Ambre et corail diffèrent aussi en luminosité, et l'erreur porte toujours un glyphe.

Valeurs complémentaires relevées dans les maquettes (hors tableau ci-dessus) :

| Usage | Sombre | Clair |
|---|---|---|
| Séparateur vertical de barre d'outils | #323238 | #DCDCE1 |
| Texte secondaire de puce inactive | #B4B4BD | #3F3F46 |
| Texte « très faible » (« ouverte », « Échap ») | #6E6E78 | n/a |
| Fond du segment actif (disposition) | #323238, texte #F2F2F5 | #FFFFFF, texte #18181B, ombre `0 1px 2px rgba(0,0,0,.12)` |
| Rail du slider / remplissage | #323238 / accent | #D4D4D8 / #1F6FEB |
| Poignée du slider | #F2F2F5, 14 px, ombre `0 1px 3px rgba(0,0,0,.5)` | #FFFFFF, 14 px, bord #C4C4CC |
| Pastille « +n nouvelles » (fond / texte) | #3A2C0C / #FFCF73 | #FFE7B3 / #6B4500 |
| En-tête de section au survol | #222227 | n/a |
| Texte de l'onglet inactif | #9696A0 | #5F5F6B |
| Fond d'onglet survolé | #1A1A1D | n/a |
| Barre de titre native | #141416, texte #C9C9D1 | #F3F3F5, texte #3F3F46 |
| Voile bandeau / pastille d'aide de l'aperçu | `rgba(12,12,13,.78)` | idem |

## 3. Typographie, espacement, hauteurs, rayons, ombres

Polices embarquées (≈ 1,1 Mo) : Inter 400 / 500 / 600 et JetBrains Mono 400 / 500. Icônes : Phosphor Regular. Segoe UI Symbol en dernier repli.

| Style | Police · taille |
|---|---|
| Heading (états vides) | Inter 600 · 16 |
| Body | Inter 400 · 13 |
| Button, onglet actif | Inter 500 · 13 |
| Barre d'état | Inter 400 · 12,5 |
| Monospace (dimensions, chemins) | JetBrains Mono 400 · 12 |
| Small (légendes) | Inter 400 · 11 |
| Badges | Inter 600 · 10 |

Échelle d'espacement : 2 · 4 · 6 · 8 · 10 · 12 · 16 · 24. Gap des tuiles : 4, sans marge extérieure (inchangé).

| Élément | Hauteur (px) |
|---|---|
| Barre de titre native (Windows) | 32 |
| Barre d'onglets / onglet | 36 / 30 |
| Barre d'outils / contrôles | 44 / 28 |
| Barre d'état | 30 |
| En-tête de section | 32 |
| Bandeau d'aperçu | 44 |
| Champ filtre (largeur) / slider (largeur) | 240 / 120 |

| Élément | Rayon |
|---|---|
| Tuile | 3 |
| Badge format | 4 |
| Boutons, champs, onglet (coins hauts) | 6 |
| Segmenté, popups, infobulle | 8 |
| Pastilles | hauteur / 2 |

| Ombre | Valeur |
|---|---|
| Popups | y 8, blur 20, alpha 128 |
| Pastille de drag | y 6, blur 14, alpha 128 |
| Tuiles | aucune (des milliers de flous par frame coûteraient cher pour rien) |

## 4. Spécification par surface

Les couleurs sont données en sombre / clair, ou par rôle de la section 2.

### 4.1 Barre de titre (fenêtre)

- Barre native conservée (pas de `decorations(false)`), accordée au thème : `ViewportCommand::SetTheme`.
- Titre : `{dossier} — Image Moz` (court), Inter 12. Le chemin complet reste en infobulle sur l'onglet. En aperçu plein cadre, la maquette affiche `{fichier} — {onglet} — Image Moz`.
- Icône d'application 16 px : mosaïque, grand rect ambre #FFB830, deux rects à droite bleu #4D94FF et blanc #E8E8EC (sombre) / noir #18181B (clair).

### 4.2 Barre d'onglets

- Hauteur 36, padding horizontal 8, gap 2, alignement bas. Fond « Barre de titre / onglets ».
- Onglet : hauteur 30, coins hauts 6 (`CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }`), padding 0 12 (0 8 0 12 quand le × est présent), gap 8 entre libellé et accessoires.
- Actif : fond « Barre d'outils, onglet actif » (raccord sans couture avec la barre d'outils), texte principal, Inter 500, × de 20 × 20 (rayon 4, couleur texte faible). Le × n'apparaît que sur l'onglet actif ou survolé ; survol du × : fond #2A2A30, texte #E8E8EC (sombre).
- Inactif : texte faible, pas de fond. Survol : fond #1A1A1D (sombre), × visible.
- Badge d'arrivées en arrière-plan : pastille ambre avec le nombre, min-width 18, hauteur 18, padding 0 5, rayon 9, texte #141416 Inter 600 11.
- Libellé tronqué à 180 px avec « … ».
- Bouton « + » (Ctrl+T) : 28 × 28, rayon 6, marge gauche 4, icône 14 px, texte faible.
- Glisser pour réordonner : libellé de l'onglet déplacé estompé à 40 %, repère d'insertion accent de 2 px × 24 px (rayon 1), pastille qui suit le curseur (hauteur 26, rayon 13, padding 0 10, fond #0B0B0C, bord #34343B, texte #F2F2F5 Inter 500, ombre « pastille de drag »).
- Glisser hors de la barre : la pastille (hauteur 30, rayon 15, padding 0 12, mêmes couleurs) affiche une icône 14 px bleu clair #8FB8FF, le nom, puis en texte faible « → nouvelle fenêtre » (détachement) ou « → fenêtre 2 » (déplacement vers une fenêtre existante).
- Risque : régressions possibles sur le drag inter-fenêtres et le détachement ; tester les 3 cas à la main.

### 4.3 Barre d'outils

Hauteur 44 (stable, jamais sur deux lignes), padding 0 10, gap 12, bordure basse 1 px « Bordure / séparateur », fond « Barre d'outils ». Contrôles de 28 de haut. Séparateurs verticaux : 1 × 20.

Ordre, de gauche à droite :

1. **Ouvrir.** Bouton 28 de haut, padding 0 10, gap 6, icône dossier 16 px, Inter 500 13. Sombre : fond #2A2A30, sans bord. Clair : fond #FFFFFF, bord 1 px #DCDCE1.
2. Séparateur.
3. **Récursif / À plat.** Deux puces (gap 4), 28 de haut, padding 0 10, rayon 6, icône 14 px + libellé. Active : fond « Contrôle actif (fond) », texte « Contrôle actif (texte) ». Inactive : transparente, texte secondaire.
4. Séparateur.
5. **Disposition** (segmenté à icônes) : rail padding 2, gap 2, rayon 8, fond « Champ texte, rail segmenté ». Trois segments de 32 × 26, rayon 6, icônes 16 px : Grille (touche 1), Justifié (touche 2), Bento (touche 3). Actif : sombre fond #323238 texte #F2F2F5 ; clair fond blanc, texte #18181B, ombre légère. Inactif : transparent, texte faible. Infobulles avec la touche (« Grille (1) », « Justifié (2) », « Bento (3) »). Raccourcis 1/2/3 et Ctrl+molette inchangés.
6. **Combo de tri** (bouton sans fond, padding 0 8 0 10, gap 6) : icône de tri 14 px, libellé Inter 500 (ex. « Nom A→Z », « Date ↓ »), chevron 10 px.
7. **Combo Bento « Grandes : »** (même gabarit, visible en Bento) : préfixe « Grandes : » en texte faible Inter 400, puis la valeur (ex. « Récentes »), chevron.
8. Espaceur flexible.
9. **Taille** : icône petit carré 12 px, slider 120 px de large (rail 4 de haut, rayon 2, remplissage accent ; poignée ronde 14 px), icône grand carré 16 px, gap 8, texte faible. La valeur « 200 px » n'est pas affichée : elle apparaît dans l'infobulle du slider pendant le glisser (« Taille cible : 200 px »).
10. **Filtre** : champ 240 × 28, rayon 6, padding 0 10 (0 6 0 10 avec le bouton d'effacement), gap 8, loupe 14 px, placeholder « Filtrer par nom… ». Au repos : fond « Champ texte », bord 1 px « Bordure ». Focus ou texte saisi : bord 1 px accent (clair : plus halo `0 0 0 3px rgba(31,111,235,.15)`, fond blanc). Bouton d'effacement « × » (infobulle « Effacer le filtre ») : 18 × 18, rayon 9, fond #34343B (sombre) / #D4D4D8 (clair), glyphe 8 px, visible seulement quand le champ n'est pas vide.

Repli étroit : voir 4.11.

### 4.4 Galerie : dispositions

- Fond de galerie, gap 4, sans marge. Un dégradé de 56 px en bas de la zone (fond de galerie, de transparent à opaque) est conservé (`Mesh`, existant).
- Grille et Bento : recadrage cover centré par rectangle UV (`cover_uv`, section 5). Justifié : ratios exacts, pas de recadrage visible.
- Bento : grille 6 colonnes, lignes de 207 (maquette à 1280 px), gap 4, remplissage dense, spans de 1 à 2 colonnes et lignes.
- Justifié : rangées de hauteur variable (214, 183, 240 dans la maquette), largeur des tuiles proportionnelle à leur ratio d'aspect.

### 4.5 Tuiles : états

Tuile de référence : 200 × 200 logiques, rayon 3, remplissage cover. Tous les anneaux sont dessinés à l'intérieur (`StrokeKind::Inside`), jamais à l'extérieur, pour ne pas déborder sur le gap de 4. Rien n'est écrit sur l'image hors badges.

| État | Rendu |
|---|---|
| Chargement | Aplat « Placeholder de tuile » (sombre #1C1C20), pas de spinner. Fondu de 120 ms vers l'image à l'arrivée de la texture. |
| Chargée | Image recadrée cover, centrée (UV). |
| Erreur / écriture en cours | Fond placeholder ; triangle d'avertissement 28 px, trait 1,8, couleur « Erreur » (#FF8A5C), distinct de l'ambre ; mot court « Illisible » en texte faible 11,5, gap 8, centré. Un glyphe est toujours présent. |
| .blend sans miniature | Fond placeholder ; icône cube 28 px (trait 1,6, #6E6E78) ; libellé « pas d'aperçu » texte faible 11,5 ; le badge format « BLEND » reste. |
| Survol | Liseré intérieur 1 px blanc à 35 % + voile blanc 6 %. Infobulle après 400 ms (4.6). |
| Sélectionnée | Anneau intérieur 2 px accent + filet intérieur sombre 1 px `rgba(0,0,0,.6)` (lisible sur image claire), voile accent 10 %, pastille ✓ de 20 px (rond, fond accent, coche #0D1A2E trait 1,8) en haut à gauche à 6 px. |
| Focus clavier seul | Double anneau : blanc #F2F2F5 2 px à l'extérieur + sombre 2 px à l'intérieur (#111113, soit (17, 17, 19) dans le code). Ne se confond pas avec la sélection. |
| Sélection + focus | Blanc #F2F2F5 2 px à l'extérieur, accent 2 px à l'intérieur (4 px au total), voile accent 10 %, pastille ✓ à 8 px. |
| Nouvelle arrivée (t = 0) | Anneau ambre 4 px (`StrokeKind::Inside`) + étiquette « NOUVEAU » en haut à droite à 8 px (hauteur 18, padding 0 6, rayon 9, fond ambre, texte #141416 Inter 600 10,5). Alpha 1 → 0 sur 1,5 s, ease-out cubique. Un seul `request_repaint` par frame, pas par tuile. |
| Épinglée (touche P) | Pastille punaise 22 × 22 (rayon 11, fond noir alpha 160, glyphe blanc 12 px) en haut à droite à 6 px, dans toutes les dispositions. |

Badge format (GLB, BLEND, etc.) : en bas à gauche à 6 px, hauteur 18, padding 0 6, rayon 4, fond noir alpha 160, texte blanc Inter 600 10, espacement des lettres 0,04 em ; icône cube 10 px facultative devant le libellé (maquette Bento).

Divergence maquette claire : l'anneau de sélection y est dessiné à 4 px accent (#1F6FEB) + filet blanc 70 %, et l'anneau d'arrivée en #E09600 à 4 px. Le code de la section 5 (2 px) fait foi pour la sélection ; l'ambre suit la colonne « Clair ».

### 4.6 Infobulle de survol (tuile)

- Largeur 264, padding 8, rayon 8, fond « Popups, infobulles », bord 1 px (#34343B en sombre), ombre « popups ».
- Zone image : hauteur 150, rayon 4, fond de galerie, miniature entière (contain) centrée. Même texture 512 px, aucun coût de décodage. Texture pas encore chargée : afficher le texte seul.
- Texte (padding 0 4 2, gap 3) : nom du fichier Inter 600 (#F2F2F5 en sombre) ; ligne mono 11,5 (ex. « 48 212 triangles · 3,4 Mo ») ; chemin relatif texte faible 12.
- Délai d'apparition : 400 ms.

### 4.7 En-têtes de section (Récursif)

- Hauteur 32, pleine largeur, padding 0 12, gap 8, fond « Barre d'état, en-têtes » (maquette claire : #EBEBEE).
- Contenu : chevron 10 px (bas = ouverte, droite = repliée) ; chemin relatif en mono 12,5 poids 500 ; compteur « n images » en texte faible (« 31 images · repliée » en maquette claire) ; pastille « +n nouvelles » (hauteur 18, padding 0 6, rayon 9, Inter 600 11, couleurs en section 2) ; espaceur ; état « ouverte » / « repliée » en #6E6E78 12 (maquette sombre).
- Survol : fond #222227 (sombre), chevron en texte principal.
- Collants : l'en-tête de la section visible reste épinglé en haut du `ScrollArea` pendant le défilement de sa section. `scroll_to` et les flèches tiennent compte des 32 px masqués.
- « +n nouvelles » : compteur remis à zéro comme le badge d'onglet.
- Section repliée : en-tête seul, ses tuiles ne sont pas affichées.

### 4.8 Barre d'état

Hauteur 30, padding 0 12, gap 14, fond « Barre d'état », bordure haute 1 px, Inter 12,5. Ordre exact, de gauche à droite :

1. **Témoin « En direct »** : point 7 × 7 (rayon 4) ambre + libellé texte faible, gap 6. Variante aperçu : « En direct · 2 nouvelles depuis l'ouverture ».
2. Séparateur vertical 1 × 14 (#323238 sombre, #D4D4D8 clair).
3. **Nom du fichier** : texte principal, poids 500 (#F2F2F5 en sombre).
4. **Dimensions** (ex. « 1153 × 1556 ») : mono 12, #C9C9D1 (sombre) / #3F3F46 (clair).
5. **Poids** (ex. « 248,8 Ko ») : mono 12, mêmes couleurs.
6. **Chemin relatif** : texte faible.
7. Espaceur flexible.
8. **Compteur** : « 328 images » (#C9C9D1 / #3F3F46) ; avec un filtre actif : « 17 / 48 images (filtre) ».
9. **Sélection** : « 1 sélectionnée » en accent (#8FB8FF en sombre, #1F6FEB en clair).

### 4.9 Aperçu plein cadre

La barre d'outils de la galerie est masquée ; barre de titre, onglets (onglet actif : fond d'aperçu #121212, icône image 12 px, Inter 500) et barre d'état restent. Zone d'aperçu : fond #121212 dans les deux thèmes. Barre d'état : celle de 4.8 (« 328 images », « 1 sélectionnée »).

**Bandeau du haut** (toujours visible) : hauteur 44, `rgba(12,12,13,.78)` (`rect_filled` noir alpha ~200, pas de flou), padding 0 12, gap 12. Ordre :

1. Bouton « Galerie » : 28 de haut, padding 0 10 0 6, rayon 6, chevron gauche 14 px, texte #C9C9D1 Inter 500 13, puis « Échap » en mono 11,5 #6E6E78.
2. Séparateur 1 × 18 (#34343B).
3. Nom du fichier : #F2F2F5 Inter 600.
4. Dimensions et poids : mono 12 texte faible (« 2048 × 1365 · 1,9 Mo »).
5. Chemin relatif : texte faible.
6. Espaceur.
7. Position « 17 / 328 » : mono 12 #C9C9D1.
8. Séparateur.
9. Groupe zoom : rail padding 2, rayon 7, fond #1E1E22 ; boutons « − » et « + » de 26 × 24 (rayon 5, #C9C9D1) autour de la valeur « 46 % » (mono 12 #F2F2F5, min-width 52, centrée).
10. Bouton « Ajuster » + « 0 » (mono texte faible) : 28 de haut, padding 0 10, rayon 6, fond #2A2A30, texte #E8E8EC Inter 500 12,5.

Les clics sur le bandeau sont consommés pour ne pas déclencher le pan.

**Flèches** : boutons ronds 44 × 44 (rayon 22), `rgba(12,12,13,.6)`, chevron 18 px trait 1,8 #E8E8EC, à 16 px des bords gauche et droit, centrés verticalement ; infobulles « Image précédente (←) », « Image suivante (→) ».

**Pastille d'aide** : centrée, à 16 px du bas, hauteur 30, padding 0 14, rayon 15, `rgba(12,12,13,.78)`, texte #C9C9D1 12, gap 14, touches en #F2F2F5 poids 600. Contenu image : « ← → précédente / suivante », « Molette zoom au curseur », « Glisser déplacer », « 0 ajuster », « Espace fermer ». Disparaît après 2 s sans souris (fondu 150 ms), réapparaît au mouvement. Le bandeau, lui, reste toujours visible.

**Variante 3D** : même chrome, contrôles caméra à la place du zoom.
- Bandeau : « Galerie », séparateur, nom (ex. « room_shell_v14.glb »), « 48 212 tri · 3 / 328 » (mono 12 texte faible), espaceur, groupe segmenté « Y-up / Z-up » (infobulle « Axe vertical (U) » ; rail padding 2, gap 2, rayon 7, fond #1E1E22 ; segment 24 de haut, padding 0 10, rayon 5, Inter 500 12 ; actif fond #34343B texte #F2F2F5, inactif texte faible), puis bouton « Recentrer » + « 0 » (même gabarit que « Ajuster »).
- Pastille d'aide, à 14 px du bas : « Clic milieu orbite », « Maj + clic milieu déplacer », « Molette rapprocher », « U Y/Z-up », « ← → modèle suivant ».

### 4.10 États vides

Fond de la maquette : #0B0B0C autour, zone #111113 (à dériver du fond de galerie du thème).

- **Fenêtre vide** (dernier onglet fermé) : barre d'onglets 36 (fond #141416) avec seulement le bouton « + » (28 × 28, marge basse 4). Au centre, gap 14 : logo mosaïque 56 px en contour (trait #3A3A42, 0,8), titre « Aucun dossier ouvert » (Inter 600 16), bouton primaire « Ouvrir un dossier » + « Ctrl+T » (hauteur 36, padding 0 16, rayon 8, fond accent, texte #0D1A2E Inter 600 13,5, icône dossier 16 px, raccourci mono 12 poids 500 opacité 0,7), puis ligne d'aide texte faible 12,5 : « ou clic droit sur un dossier dans l'Explorateur › Ouvrir dans Image Moz ».
- **Filtre sans résultat** : barre d'outils (44, fond #1E1E22) avec le champ de filtre à droite (bord accent, texte saisi, bouton d'effacement). Au centre, gap 12 : loupe 36 px (trait #4A4A54, 1,1), titre « Aucun fichier ne correspond au filtre » (Inter 600 15), ligne texte faible 12,5 de la forme « « chaise_v9 » · 0 sur 328 images », bouton secondaire « Effacer le filtre » (hauteur 30, padding 0 12, rayon 6, fond #2A2A30, texte #E8E8EC Inter 500 13).

### 4.11 Fenêtre étroite et mise à l'échelle

- Sous 1 100 px de large : les deux icônes du slider disparaissent et le filtre passe à 180 px.
- Sous 960 px : « Récursif » et « À plat » deviennent des icônes seules (infobulle avec le libellé) et « Grandes : » est retiré du combo Bento.
- Plus étroit, la barre se dégrade selon la largeur réellement disponible, dans cet ordre : tri en icône + chevron (libellé en infobulle) ; slider à 80 px ; filtre à 140 px ; combo Bento en icône seule (infobulle « Grandes : … ») ; slider masqué (Ctrl+molette redimensionne toujours) ; « Ouvrir » en icône seule ; filtre à 100 px ; séparateurs supprimés. Le groupe slider + filtre est posé dans son propre rectangle à droite : il ne recouvre jamais les groupes de gauche, qui sont rognés si rien ne tient. Largeur minimale de fenêtre : 480 pt.
- La barre d'outils ne passe jamais sur deux lignes : la hauteur de 44 reste stable pour éviter un saut de la galerie.
- Tout est en points logiques : à 200 %, egui double ; les miniatures 512 px couvrent une tuile de 256 logiques sans flou.

### 4.12 Animations

| Animation | Durée |
|---|---|
| Fondu chargement → image | 120 ms |
| Anneau d'arrivée | 1,5 s, ease-out |
| Pastille d'aide de l'aperçu | 150 ms |

Toutes via `animate_value_with_time`, courtes ; le repaint s'arrête quand plus rien ne bouge.

## 5. Mapping egui

Noms de champs alignés sur egui 0.31+ (`CornerRadius` en u8, `Shadow` en entiers) ; à vérifier à la compilation contre 0.35. Le thème clair est celui par défaut : `light_visuals()` reprend ce squelette avec la colonne « Clair » (base `egui::Visuals::light()`), et `ctx.set_visuals(light_visuals())` au démarrage tant que l'utilisateur n'a pas choisi « Sombre » ou « Système ».

```rust
use egui::{Color32 as C, CornerRadius, Shadow, Stroke, FontId, FontFamily, TextStyle, vec2};

pub fn dark_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill        = C::from_rgb(30, 30, 34);   // barre d'outils, onglet actif
    v.window_fill       = C::from_rgb(38, 38, 43);   // popups, combos, infobulles
    v.extreme_bg_color  = C::from_rgb(20, 20, 22);   // champ filtre, rail segmenté
    v.faint_bg_color    = C::from_rgb(24, 24, 27);   // barre d'état, en-têtes
    v.hyperlink_color   = C::from_rgb(143, 184, 255);
    v.selection.bg_fill = C::from_rgb(34, 50, 77);   // puces/segments actifs
    v.selection.stroke  = Stroke::new(1.0, C::from_rgb(184, 210, 255));
    v.window_stroke     = Stroke::new(1.0, C::from_rgb(52, 52, 59));
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius   = CornerRadius::same(8);
    v.popup_shadow  = Shadow { offset: [0, 8], blur: 20, spread: 0, color: C::from_black_alpha(128) };
    v.window_shadow = v.popup_shadow;

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, C::from_rgb(42, 42, 48));
    w.noninteractive.fg_stroke = Stroke::new(1.0, C::from_rgb(232, 232, 236));
    w.inactive.weak_bg_fill = C::TRANSPARENT;          // boutons fantômes par défaut
    w.inactive.bg_fill      = C::from_rgb(50, 50, 56); // rail du slider, case à cocher
    w.inactive.fg_stroke    = Stroke::new(1.0, C::from_rgb(200, 200, 208));
    w.hovered.weak_bg_fill  = C::from_rgb(42, 42, 48);
    w.hovered.bg_stroke     = Stroke::NONE;
    w.active.weak_bg_fill   = C::from_rgb(50, 50, 56);
    w.active.bg_stroke      = Stroke::NONE;
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = CornerRadius::same(6);
        s.expansion = 0.0;                                // pas de « gonflement » au survol
    }
    v
}

pub fn apply_style(ctx: &egui::Context) {
    ctx.style_mut(|s| {
        let p = FontFamily::Proportional;
        let medium = FontFamily::Name("medium".into());
        s.text_styles = [
            (TextStyle::Small,     FontId::new(11.0, p.clone())),
            (TextStyle::Body,      FontId::new(13.0, p.clone())),
            (TextStyle::Button,    FontId::new(13.0, medium)),
            (TextStyle::Heading,   FontId::new(16.0, FontFamily::Name("semibold".into()))),
            (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        ].into();
        s.spacing.item_spacing   = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.interact_size  = vec2(28.0, 28.0);
        s.spacing.slider_width   = 120.0;
    });
}
```

```rust
// Polices : Inter en tête de Proportional, familles nommées pour 500/600, Phosphor pour les icônes.
let mut f = egui::FontDefinitions::default();
f.font_data.insert("inter".into(),    Arc::new(FontData::from_static(include_bytes!("../assets/Inter-Regular.ttf"))));
f.font_data.insert("inter-500".into(),Arc::new(FontData::from_static(include_bytes!("../assets/Inter-Medium.ttf"))));
f.font_data.insert("inter-600".into(),Arc::new(FontData::from_static(include_bytes!("../assets/Inter-SemiBold.ttf"))));
f.font_data.insert("mono".into(),     Arc::new(FontData::from_static(include_bytes!("../assets/JetBrainsMono-Regular.ttf"))));
f.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "inter".into());
f.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, "mono".into());
f.families.insert(FontFamily::Name("medium".into()),   vec!["inter-500".into(), "inter".into()]);
f.families.insert(FontFamily::Name("semibold".into()), vec!["inter-600".into(), "inter".into()]);
egui_phosphor::add_to_fonts(&mut f, egui_phosphor::Variant::Regular); // vérifier la version compatible 0.35
ctx.set_fonts(f);

// Tuile : recadrage cover = un simple rectangle UV, aucun coût GPU supplémentaire.
fn cover_uv(img: egui::Vec2, cell: egui::Vec2) -> egui::Rect {
    let (ia, ca) = (img.x / img.y, cell.x / cell.y);
    if ia > ca { let k = ca / ia; egui::Rect::from_min_max(egui::pos2((1.0 - k) / 2.0, 0.0), egui::pos2((1.0 + k) / 2.0, 1.0)) }
    else       { let k = ia / ca; egui::Rect::from_min_max(egui::pos2(0.0, (1.0 - k) / 2.0), egui::pos2(1.0, (1.0 + k) / 2.0)) }
}
egui::Image::from_texture(&tex).uv(cover_uv(tex.size_vec2(), rect.size())).corner_radius(3).paint_at(ui, rect);

// Anneaux à l'intérieur de la cellule.
let p = ui.painter();
if selected { p.rect_filled(rect, 3, accent.gamma_multiply(0.10));
              p.rect_stroke(rect, 3, Stroke::new(2.0, accent), egui::StrokeKind::Inside); }
if focused  { p.rect_stroke(rect, 3, Stroke::new(2.0, C::from_rgb(242, 242, 245)), egui::StrokeKind::Inside);
              p.rect_stroke(rect.shrink(2.0), 1, Stroke::new(2.0, if selected { accent } else { C::from_rgb(17, 17, 19) }), egui::StrokeKind::Inside); }
if let Some(t) = arrived_secs_ago.filter(|t| *t < 1.5) {
    let a = 1.0 - ease_out_cubic(t / 1.5);
    p.rect_stroke(rect, 3, Stroke::new(4.0, AMBER.gamma_multiply(a)), egui::StrokeKind::Inside);
    any_fading = true;   // un seul ctx.request_repaint() après la boucle, pas un par tuile
}

// Titre court ; thème clair par défaut, barre de titre accordée.
ctx.set_visuals(light_visuals());
ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{folder} — Image Moz")));
ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Light));
```

Répartition style / painter :

- **Style seul (Visuals / Spacing)** : boutons fantômes, combos, slider, case à cocher, champ filtre, popups, infobulles, menu contextuel, barre d'état.
- **Painter custom** : tuiles et leurs états, onglets (coins hauts seuls via `CornerRadius { nw, ne, sw: 0, se: 0 }`), pastilles de badge, repère d'insertion, pastille de drag, segmenté (Frame + `selectable_label`), en-têtes collants, bandeau et pastille d'aide de l'aperçu (`rect_filled` noir alpha 200, pas de flou), dégradé bas (`Mesh`, existant).
- **Crates** : `egui-phosphor` (icônes). Aucune autre : pas de SVG à l'exécution.
- **Animations** : voir 4.12.

## 6. Plan

### Niveau 1 : style et polices (≈ 1 jour)

| Quoi | Coût | Risque |
|---|---|---|
| Visuals clair (par défaut) + sombre à partir des tokens, choix Clair / Sombre / Système | 2 h | Risque nul ; mémoriser le choix avec la session. |
| Inter + JetBrains Mono + Phosphor, échelle de texte | 2 h | Binaire +1,1 Mo. Vérifier la version d'egui-phosphor compatible 0.35. |
| Placeholder et fond de galerie issus du thème (fin du gris 44) | 15 min | Aucun. |
| Recadrage cover en Grille et Bento | 1 h | Perf nulle (UV). Les collages perdent leurs bords : compensé par l'infobulle (niveau 2). |
| Anneaux sélection / focus / arrivée redessinés, `StrokeKind::Inside` | 1 h | Garder un seul `request_repaint` pour le fondu. |
| Titre court + barre de titre sombre | 30 min | Le chemin UNC disparaît du titre : il reste au survol de l'onglet. |
| Barre d'état alignée à gauche, tailles, mono, témoin « En direct » | 1 h | Aucun ; mise en page linéaire. |

### Niveau 2 : changements structurels (≈ 4 à 6 jours)

| Quoi | Coût | Risque |
|---|---|---|
| Barre d'onglets custom : raccord, × au survol, badge, +, pastille de drag | 1 j | Régressions possibles sur le drag inter-fenêtres et le détachement : tester les 3 cas à la main. |
| Barre d'outils groupée, segmenté à icônes, repli sous 1 100 / 960 px | 0,5 à 1 j | Faible. Raccourcis 1/2/3 et Ctrl+molette inchangés ; infobulles avec la touche. |
| Infobulle avec miniature entière (contain) | 0,5 j | Texture pas encore chargée : afficher le texte seul. |
| Punaise partout, « +n nouvelles » sur les en-têtes | 0,5 j | Persistance : remettre le compteur à zéro comme le badge d'onglet. |
| En-têtes de section collants | 0,5 à 1 j | `scroll_to` et les flèches doivent tenir compte des 32 px masqués. |
| Chrome de l'aperçu : bandeau, flèches, aide auto-masquée, barre d'outils cachée | 1 j | Clics sur le bandeau vs pan : consommer l'entrée sur la zone du bandeau. |
| Écran vide et filtre vide | 2 h | Aucun. |

### Écarté

Barre de titre personnalisée (`decorations(false)`) : il faudrait refaire le glisser, les bords de redimensionnement, le double-clic et les snap layouts de Windows 11, pour un gain minime une fois le titre raccourci et passé en sombre.
