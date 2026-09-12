#!/usr/bin/env bash
set -euo pipefail

SCREENSHOT_DIR="./screenshots"

echo "==> Verification des dependances (cage, grim)..."
command -v cage >/dev/null 2>&1 || { echo "Erreur: 'cage' n'est pas installe."; exit 1; }
command -v grim >/dev/null 2>&1 || { echo "Erreur: 'grim' n'est pas installe."; exit 1; }

# Gestion des arguments pour les delais de capture (par defaut: 2.5 secondes)
DELAYS=("$@")
if [ ${#DELAYS[@]} -eq 0 ]; then
    DELAYS=(2.5)
fi

mkdir -p "${SCREENSHOT_DIR}"

echo "==> Compilation du projet Rust..."
cargo build

echo "==> Lancement de l'app sous cage (mode headless wlroots)..."
WLR_BACKENDS=headless WAYLAND_DISPLAY= cage -- cargo run &
CAGE_PID=$!

# Nettoyage garanti à la sortie
trap 'kill "${CAGE_PID}" 2>/dev/null || true' EXIT

# Attente initiale pour s'assurer que le socket Wayland est disponible
sleep 1

# Recuperation du socket Wayland cree par cage
NESTED_SOCKET=$(ls -t "${XDG_RUNTIME_DIR}"/wayland-* 2>/dev/null | head -n 1 | xargs basename)

if [ -z "${NESTED_SOCKET}" ]; then
    echo "==> Echec : Impossible de trouver le socket Wayland cree par cage."
    exit 1
fi

# Trier les delais et effectuer les captures
PREV_DELAY=0
for DELAY in $(echo "${DELAYS[@]}" | tr ' ' '\n' | sort -n); do
    # Calcul de l'attente relative entre deux captures
    SLEEP_TIME=$(awk "BEGIN {print ${DELAY} - ${PREV_DELAY}}")
    if (( $(echo "${SLEEP_TIME} > 0" | bc -l) )); then
        sleep "${SLEEP_TIME}"
    fi

    OUT_FILE="${SCREENSHOT_DIR}/screenshot_${DELAY}s.png"
    echo "==> Capture d'ecran a t=${DELAY}s -> ${OUT_FILE}..."
    WAYLAND_DISPLAY="${NESTED_SOCKET}" grim "${OUT_FILE}"
    PREV_DELAY="${DELAY}"
done

echo "==> Nettoyage..."
kill "${CAGE_PID}" 2>/dev/null || true

echo "==> Termine. Captures disponibles dans ${SCREENSHOT_DIR}/"
