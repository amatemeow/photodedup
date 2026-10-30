# sidecar

The ffmpeg and ffprobe programs that ship inside the app. The binaries
(`photodedup-ffmpeg-<target-triple>`, `photodedup-ffprobe-<target-triple>`) are **built, not
committed**: run `scripts/build-sidecar.sh`. It builds a pinned ffmpeg source
tarball as an LGPL build and refuses to finish if the result isn't
redistributable (`photodedup check-sidecar`). `SOURCE.txt` records exactly what
was built.

Why this exists: ffmpeg's licence depends on how it was configured. Most
prebuilt binaries include GPL components, and shipping one would make the whole
app GPL. photodedup runs ffmpeg as a separate program and never links it, and
anyone can substitute their own build (`PHOTODEDUP_FFMPEG`, `PHOTODEDUP_FFPROBE`,
or a file placed beside the app).
