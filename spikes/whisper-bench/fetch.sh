#!/usr/bin/env bash
# Fetches the benchmark inputs (about 295 MB) and cuts each speech sample to 600 s of
# 16 kHz mono WAV. Re-running skips files that are already present. Bytes actually
# transferred are logged to out/downloads.tsv.
set -euo pipefail

cd "$(dirname "$0")"
mkdir -p models audio out

readonly HF=https://huggingface.co
readonly LOG=out/downloads.tsv
readonly CLIP_SECONDS=600

# path|url|expected bytes (optional)|sha256 (optional)
readonly FILES=(
  "models/ggml-tiny-q5_1.bin|$HF/ggerganov/whisper.cpp/resolve/main/ggml-tiny-q5_1.bin|32152673|818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7"
  "models/ggml-base-q5_1.bin|$HF/ggerganov/whisper.cpp/resolve/main/ggml-base-q5_1.bin|59707625|422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898"
  "models/ggml-small-q5_1.bin|$HF/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin|190085487|ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb"
  "models/ggml-silero-v6.2.0.bin|$HF/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin|885098|2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987"
  "audio/en-alice-ch01.mp3|https://archive.org/download/alice_in_wonderland_librivox/wonderland_ch_01_64kb.mp3||"
  "audio/fr-cornille.mp3|https://archive.org/download/lettres_de_mon_moulin_naf_0901_librivox/lettresdemonmoulin_03_daudet_64kb.mp3||"
  "audio/en-alice.gutenberg.txt|https://www.gutenberg.org/cache/epub/11/pg11.txt||"
  "audio/fr-lettres.gutenberg.txt|https://www.gutenberg.org/cache/epub/36780/pg36780.txt||"
)

part_size() {
  if [[ -f $1 ]]; then stat -c %s "$1"; else echo 0; fi
}

# Resumes into "$path.part" on each attempt and logs the bytes each attempt added,
# so interrupted transfers are counted once and never restarted from zero.
download() {
  local path=$1 url=$2 attempt before status=1
  for attempt in 1 2 3 4 5; do
    before=$(part_size "$path.part")
    status=0
    curl -fsSL --http1.1 -C - -o "$path.part" "$url" || status=$?
    printf '%s\t%s\n' "$path" "$(($(part_size "$path.part") - before))" >>"$LOG"
    if ((status == 0)); then
      return
    fi
    echo "retry $attempt for $path (curl exit $status)" >&2
    sleep 3
  done
  echo "giving up on $path" >&2
  exit 1
}

fetch() {
  local path=$1 url=$2 bytes=$3 sha=$4
  if [[ -s $path ]]; then
    echo "have $path"
    return
  fi
  echo "get  $path"
  download "$path" "$url"
  if [[ -n $bytes && $(stat -c %s "$path.part") != "$bytes" ]]; then
    echo "size mismatch for $path" >&2
    exit 1
  fi
  if [[ -n $sha ]]; then
    echo "$sha  $path.part" | sha256sum --check --quiet -
  fi
  mv "$path.part" "$path"
}

to_wav() {
  local src=$1 dst=$2
  if [[ -s $dst ]]; then
    echo "have $dst"
    return
  fi
  ffmpeg -nostdin -loglevel error -y -i "$src" -t "$CLIP_SECONDS" -ac 1 -ar 16000 -c:a pcm_s16le "$dst"
  echo "made $dst"
}

for entry in "${FILES[@]}"; do
  IFS='|' read -r path url bytes sha <<<"$entry"
  fetch "$path" "$url" "$bytes" "$sha"
done

to_wav audio/en-alice-ch01.mp3 audio/en.wav
to_wav audio/fr-cornille.mp3 audio/fr.wav

if [[ -f $LOG ]]; then
  awk -F'\t' '{ total += $2 } END { printf "downloaded so far: %d bytes (%.1f MB)\n", total, total / 1e6 }' "$LOG"
fi
