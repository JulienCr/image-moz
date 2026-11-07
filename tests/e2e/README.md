# Tests de bout en bout

Ces scripts lancent le vrai `image-moz.exe` (build debug), déplacent la vraie souris, tapent au vrai
clavier et vérifient ce que l'application fait : fenêtres, onglets, session enregistrée, journal,
fin du processus. Ils couvrent les onglets, les fenêtres multiples (réordonner, détacher, fusionner,
fermer), l'instance unique, la restauration de session et les raccourcis clavier.

## À savoir avant de lancer

- Il faut une **session Windows interactive** (bureau déverrouillé, écran allumé) et **PowerShell 7**
  (`pwsh`).
- Le test **prend le contrôle de la souris et du clavier**. Ne touchez à rien pendant qu'il tourne,
  sinon les glisser-déposer échouent. Comptez environ une minute pour tout lancer.
- Rien n'est partagé avec votre installation : chaque scénario utilise son propre dossier de
  configuration, son propre cache de miniatures et son propre nom d'instance (variables
  `IMAGE_MOZ_CONFIG_DIR`, `IMAGE_MOZ_CACHE_DIR` et `IMAGE_MOZ_INSTANCE`). Votre
  `%APPDATA%\image-moz\config.json` n'est ni lu ni écrit. `run.ps1` le vérifie à la fin (empreinte
  avant et après) et signale un échec si le fichier a changé.
- Les images de test sont générées dans un dossier temporaire, supprimé à la fin.

## Lancer

Depuis la racine du dépôt :

```powershell
pwsh tests\e2e\run.ps1                  # compile (cargo build) puis lance tous les scénarios
pwsh tests\e2e\run.ps1 detach merge     # seulement ceux-là
pwsh tests\e2e\run.ps1 -List            # liste des scénarios
pwsh tests\e2e\run.ps1 -NoBuild         # réutilise target\debug\image-moz.exe
```

La sortie affiche `PASS` ou `FAIL` par scénario, puis un total. Le code de sortie est non nul dès
qu'un scénario échoue.

## Diagnostic

Le dossier `tests\e2e\out\` (ignoré par git) est recréé à chaque lancement. Il contient le journal de
chaque scénario (`<scénario>.log`, niveau debug) et, pour un scénario en échec, une capture d'écran
`<scénario>-failure.png`. Les captures servent uniquement à comprendre un échec : aucune assertion
ne s'appuie dessus.

## Ajouter un scénario

Créer `tests\e2e\scenarios\NN-nom.ps1`, qui commence par `param([string]$Fix, [string]$Name)`, puis
utilise les fonctions de `lib.ps1` : `New-AppEnv`, `Start-Session`, `Get-AppWindows`, `Get-Tabs`,
`Drag-Mouse`, `Send-Chord`, `Wait-SavedConfig`, `Assert-Equal`… Les positions se déduisent des
rectangles de fenêtres et des onglets (que l'application écrit dans son journal de debug), jamais de
coordonnées écrites en dur. Une exception levée fait échouer le scénario.
