# Contexte — Problèmes & Solutions

> **`CLAUDE.md` (racine) fait autorité.** Ce fichier est un journal court : un
> problème rencontré, sa cause, ce qui l'a réglé. Quand une ligne ci-dessous est
> marquée *(remplacé)*, le mécanisme cité n'existe plus dans le code — la
> solution actuelle est décrite dans `CLAUDE.md`. Ne jamais réintroduire un
> mécanisme remplacé en se fiant à cette table seule.

| Problème | Cause | Solution |
|----------|-------|----------|
| Clips 120fps corrompus (0.17s, 72000+ fps) | gdigrab ne peut pas capturer au-dessus de ~60fps | Switch vers ddagrab (DXGI Desktop Duplication) pour 120/240fps |
| Souris qui clignote | 6 processus FFmpeg orphelins tournaient en parallèle (anciens crashes) | Mutex single-instance + `-draw_mouse 0` + cleanup au startup — qui tue désormais le **processus** orphelin (PID relu depuis `ffmpeg.pid`), pas seulement son dossier temp : un ffmpeg orphelin retient une session NVENC et quelques-uns suffisent à ce que plus rien ne s'encode |
| Désync audio/vidéo *(remplacé)* | ~~Audio buffer capté jusqu'au save, mais vidéo finit au dernier segment complet~~ — la vraie cause était que la durée du buffer audio était déduite d'un comptage d'octets, faux dès que le loopback WASAPI sous-livre pendant les silences | ~~`end_offset = time.time() - last_segment_mtime`~~ supprimé. Aujourd'hui : chunks horodatés + `_render_window()` qui reconstruit la fenêtre sur l'horloge murale réelle (round 6 dans `CLAUDE.md`) |
| Pas de micro dans les clips | AudioCapture ne capturait que le loopback système | Ajout dual-stream : loopback + mic avec buffers/locks séparés, amix pour le merge |
| ddagrab + format=yuv420p → erreur | NVENC gère le pixel format en interne, le filtre format est incompatible | Retirer `-vf format=yuv420p` quand on utilise NVENC |
| Buzz/grésillage audio *(partiellement remplacé)* | ~~`_get_pcm` coupait le buffer sans aligner sur les frame boundaries~~ (fonction supprimée) + buffer 1024 trop petit + amix normalisait | `frames_per_buffer=4096` et `normalize=0` tiennent toujours. Le grésillage restant venait de `_render_window` qui perçait un trou de 1–5 ms à chaque frontière de chunk : corrigé par une tolérance de 30 ms (round 7), mesuré 38 → 0 discontinuités |
| Clip qui s'arrête avant l'appui sur la touche | Le segment en cours est illisible (frame déchirée) donc exclu : on perd jusqu'à `SEGMENT_DURATION` | `SEGMENT_DURATION` 5 → 1, perte mesurée 1.9 s → 0.87 s |
| Clip corrompu juste après un changement de réglage | Le segment en cours d'écriture était quand même sélectionné et snapshotté | `save_replay()` construit `complete = files_with_mtime[:-1]` et n'y touche plus ; abandon propre s'il est vide |
| PyInstaller PermissionError | L'exe était encore lancé pendant le build | Fermer ClipRecorder.exe avant le build. Préférer `taskkill` **sans** `/F` : un kill forcé saute l'arrêt propre de l'app et abandonne son ffmpeg enfant (voir la ligne « souris qui clignote ») |
| Boutons settings cachés | Fenêtre trop petite après ajout de lignes | Agrandie — actuellement `420x510` (`SettingsWindow._build`) |
| `pyaudio` not found | Le projet utilise `pyaudiowpatch` (pas pyaudio standard) importé comme `pyaudio` | Installer `pyaudiowpatch` et non `pyaudio` |

## Skills disponibles

- `/context` — afficher/ajouter des problèmes et solutions dans ce fichier
- `/build-exe` — build PyInstaller (single exe portable)
- `/clean-repo` — préparer le repo pour une release publique sur GitHub (audit secrets, LICENSE, README, release, visibilité)
