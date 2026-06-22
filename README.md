# Clip Recorder

Enregistreur d'écran "replay" — capture en continu, sauvegarde les dernières X secondes en MP4 avec le son. Léger, pas de bloatware.

## Utilisation

Lancer `ClipRecorder.exe` — la capture démarre automatiquement en arrière-plan.
Une icône rouge apparaît dans la barre des tâches (system tray).

**`Ctrl+Alt+R`** → Sauvegarde un clip des dernières X secondes.

Clic droit sur l'icône pour : sauver le clip, paramètres, ouvrir le dossier, quitter.

## Audio

L'audio système est capturé automatiquement via WASAPI loopback (pyaudiowpatch).
Aucune configuration nécessaire — le son des haut-parleurs par défaut est enregistré.

L'encodeur GPU (NVENC) est utilisé automatiquement si disponible.

## Recompiler

Nécessite Python 3 + `py -m pip install -r requirements.txt`

```
build.bat
```

L'exe sort dans `dist\ClipRecorder.exe`.

## Licence

[MIT](LICENSE)
