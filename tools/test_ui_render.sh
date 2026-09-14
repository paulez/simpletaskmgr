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
# Cage reçoit son propre XDG_RUNTIME_DIR : le socket Wayland y est alors
# sans ambiguïté (wayland-0), même si l'utilisateur a une session desktop
# active sur sa plage de sockets (sa session occupe déjà /run/user/UID/wayland-0
# et la détection "socket le plus récent" choisissait le socket de GNOME).
# DBUS_SESSION_BUS_ADDRESS=disabled coupe la prise de la session GDBus de
# l'utilisateur : sans bus de session, GApplication ne détecte pas une
# instance primaire déjà ouverte et démarre une instance indépendante (sinon
# l'app dans cage se termine aussitôt et cage la suit).
CAGE_RUNTIME="$(mktemp -d "${TMPDIR:-/tmp}/cage-XXXXXX")"
WLR_BACKENDS=headless WAYLAND_DISPLAY= XDG_RUNTIME_DIR="${CAGE_RUNTIME}" DBUS_SESSION_BUS_ADDRESS=disabled cage -- cargo run &
CAGE_PID=$!

# Nettoyage garanti à la sortie
trap 'kill "${CAGE_PID}" 2>/dev/null || true; rm -rf "${CAGE_RUNTIME}"' EXIT

# Socket Wayland de cage, dans son runtime dir dédié :
NESTED_SOCKET="wayland-0"

# Attente (bornée) que le socket Wayland de cage existe : au démarrage,
# cage peut mettre plus d'1 s (retour de la prise GPU vers le backend
# headless).
tested=0
for _ in $(seq 1 50); do
    if [ -S "${CAGE_RUNTIME}/${NESTED_SOCKET}" ]; then
        tested=1
        break
    fi
    sleep 0.2
done

if [ "${tested}" -eq 0 ]; then
    echo "==> Echec : le socket Wayland de cage n'existe pas (10 s d'attente)."
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
    WAYLAND_DISPLAY="${NESTED_SOCKET}" XDG_RUNTIME_DIR="${CAGE_RUNTIME}" grim "${OUT_FILE}"
    PREV_DELAY="${DELAY}"
done

echo "==> Nettoyage..."
kill "${CAGE_PID}" 2>/dev/null || true

echo "==> Termine. Captures disponibles dans ${SCREENSHOT_DIR}/"
