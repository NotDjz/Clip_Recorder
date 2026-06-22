# Clip Recorder

Enregistreur d'écran "replay" — capture en continu, sauvegarde les dernières X secondes en MP4 via un raccourci clavier. Léger, pas de bloatware.

## Utilisation

Lancer `ClipRecorder.exe` — la capture démarre automatiquement.
Un indicateur "REC" apparaît en haut à droite de l'écran.

## Raccourcis

| Raccourci | Action |
|-----------|--------|
| `Ctrl+Alt+R` | Sauver le replay (dernières X secondes) |
| `Ctrl+Alt+P` | Pause / reprendre la capture |
| `Ctrl+Alt+S` | Ouvrir les paramètres |
| `Ctrl+Alt+Q` | Quitter |

## Paramètres

- Écran à capturer (multi-moniteur supporté)
- FPS : 15, 30 ou 60
- Qualité : basse / moyenne / haute
- Durée du buffer : 15 à 120 secondes
- Dossier de sortie

L'encodeur GPU (NVENC) est utilisé automatiquement si disponible.

## Recompiler

Nécessite Python 3 + `py -m pip install -r requirements.txt`

```
build.bat
```

L'exe sort dans `dist\ClipRecorder.exe`.

## Licence

[MIT](LICENSE)
