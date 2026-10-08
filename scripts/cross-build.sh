#!/usr/bin/env bash

set -euo pipefail

readonly PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
readonly DEFAULT_OUTPUT_DIR="${PROJECT_ROOT}/dist"

generation="all"
profile="release"
output_dir="${DEFAULT_OUTPUT_DIR}"
generic_sdk_env=""
generic_target=""
offline=false
dry_run=false
install_target=false
list_sdks=false
allow_generic=false

declare -A sdk_envs=(
    [3]="${RCAR_GEN3_SDK_ENV:-}"
    [4]="${RCAR_GEN4_SDK_ENV:-}"
    [5]="${RCAR_GEN5_SDK_ENV:-}"
)
declare -A targets=(
    [3]="${RCAR_GEN3_RUST_TARGET:-}"
    [4]="${RCAR_GEN4_RUST_TARGET:-}"
    [5]="${RCAR_GEN5_RUST_TARGET:-}"
)

usage() {
    cat <<'EOF'
Cross-compile edgeagent-rs for R-Car Gen3, Gen4, and Gen5 targets.

Usage:
  scripts/cross-build.sh [options]

Options:
  -g, --generation <3|4|5|all>  Generation to build (default: all)
      --sdk-env <path>          Yocto SDK environment file for selected builds
      --gen3-sdk-env <path>     Gen3 Yocto SDK environment file
      --gen4-sdk-env <path>     Gen4 Yocto SDK environment file
      --gen5-sdk-env <path>     Gen5 Yocto SDK environment file
      --target <triple>         Rust target triple for selected builds
      --gen3-target <triple>    Override the inferred Gen3 Rust target
      --gen4-target <triple>    Override the inferred Gen4 Rust target
      --gen5-target <triple>    Override the inferred Gen5 Rust target
      --debug                   Build the development profile
      --offline                 Pass --offline to Cargo
        --install-target          Install missing Rust std target via rustup only
        --list-sdks               List installed Yocto SDK environment files
        --allow-generic           Permit a GNU release without a Yocto SDK
  -o, --output <directory>      Bundle output directory (default: ./dist)
      --dry-run                 Validate inputs and print planned builds
  -h, --help                    Show this help

Environment equivalents:
  RCAR_GEN3_SDK_ENV, RCAR_GEN4_SDK_ENV, RCAR_GEN5_SDK_ENV
  RCAR_GEN3_RUST_TARGET, RCAR_GEN4_RUST_TARGET, RCAR_GEN5_RUST_TARGET

--install-target does not install a C cross-linker or target sysroot.
GNU release bundles require a Yocto SDK unless --allow-generic is supplied.
Generic GNU bundles can require a newer glibc than the deployed board and are
development-only. Musl release bundles must be statically linked.

The SDK environment is expected to define CC and normally AR, LDFLAGS, and
SDKTARGETSYSROOT. R-Car Linux SDKs commonly provide an environment-setup-*
file. The script maps aarch64 SDK compilers to aarch64-unknown-linux-gnu and
32-bit hard-float ARM SDK compilers to armv7-unknown-linux-gnueabihf.
EOF
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

require_value() {
    [[ $# -ge 2 && -n "${2:-}" ]] || fail "$1 requires a value"
}

while (($# > 0)); do
    case "$1" in
        -g|--generation)
            require_value "$@"
            generation="$2"
            shift 2
            ;;
        --sdk-env)
            require_value "$@"
            generic_sdk_env="$2"
            shift 2
            ;;
        --gen3-sdk-env|--gen4-sdk-env|--gen5-sdk-env)
            require_value "$@"
            sdk_envs["${1:5:1}"]="$2"
            shift 2
            ;;
        --target)
            require_value "$@"
            generic_target="$2"
            shift 2
            ;;
        --gen3-target|--gen4-target|--gen5-target)
            require_value "$@"
            targets["${1:5:1}"]="$2"
            shift 2
            ;;
        --debug)
            profile="debug"
            shift
            ;;
        --offline)
            offline=true
            shift
            ;;
        --install-target)
            install_target=true
            shift
            ;;
        --list-sdks)
            list_sdks=true
            shift
            ;;
        --allow-generic)
            allow_generic=true
            shift
            ;;
        -o|--output)
            require_value "$@"
            output_dir="$2"
            shift 2
            ;;
        --dry-run)
            dry_run=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "unknown option: $1"
            ;;
    esac
done

case "${generation}" in
    3|4|5) generations=("${generation}") ;;
    all) generations=(3 4 5) ;;
    *) fail "generation must be 3, 4, 5, or all" ;;
esac

discover_sdk_envs() {
    local roots=()
    local root
    for root in /opt /usr/local/oecore "${HOME}"; do
        [[ -d "${root}" ]] && roots+=("${root}")
    done
    ((${#roots[@]} > 0)) || return 0
    find "${roots[@]}" -maxdepth 7 -type f -name 'environment-setup-*' -print 2>/dev/null | sort -u
}

if [[ "${list_sdks}" == true ]]; then
    sdk_candidates="$(discover_sdk_envs)"
    if [[ -n "${sdk_candidates}" ]]; then
        printf '%s\n' "${sdk_candidates}"
    else
        printf '%s\n' "No Yocto SDK environment-setup-* files found under /opt, /usr/local/oecore, or ${HOME}."
    fi
    exit 0
fi

command -v cargo >/dev/null 2>&1 || fail "cargo is required"
command -v rustc >/dev/null 2>&1 || fail "rustc is required"
if [[ "${dry_run}" != true ]]; then
    command -v file >/dev/null 2>&1 || fail "file is required"
    command -v sed >/dev/null 2>&1 || fail "sed is required"
    command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is required"
fi

infer_target() {
    local compiler_name="${1,,}"
    case "${compiler_name}" in
        *aarch64*musl*) printf '%s\n' "aarch64-unknown-linux-musl" ;;
        *aarch64*) printf '%s\n' "aarch64-unknown-linux-gnu" ;;
        *arm*gnueabihf*|*arm*eabihf*) printf '%s\n' "armv7-unknown-linux-gnueabihf" ;;
        *arm*musl*) printf '%s\n' "armv7-unknown-linux-musleabihf" ;;
        *arm*gnueabi*) printf '%s\n' "armv7-unknown-linux-gnueabi" ;;
        *) return 1 ;;
    esac
}

default_compiler() {
    case "$1" in
        aarch64-unknown-linux-gnu) printf '%s\n' "aarch64-linux-gnu-gcc" ;;
        aarch64-unknown-linux-musl) printf '%s\n' "aarch64-linux-musl-gcc" ;;
        armv7-unknown-linux-gnueabihf) printf '%s\n' "arm-linux-gnueabihf-gcc" ;;
        armv7-unknown-linux-gnueabi) printf '%s\n' "arm-linux-gnueabi-gcc" ;;
        armv7-unknown-linux-musleabihf) printf '%s\n' "arm-linux-musleabihf-gcc" ;;
        *) return 1 ;;
    esac
}

missing_linker_help() {
    local target="$1"
    local compiler="$2"
    printf 'error: cross-linker not found: %s\n' "${compiler}" >&2
    printf '%s\n' "Installing a Rust target does not install its C linker or target libc." >&2
    case "${target}" in
        aarch64-unknown-linux-gnu)
            printf '%s\n' "Generic Debian/Kali option:" >&2
            printf '%s\n' "  sudo apt-get update" >&2
            printf '%s\n' "  sudo apt-get install gcc-aarch64-linux-gnu libc6-dev-arm64-cross" >&2
            ;;
        armv7-unknown-linux-gnueabihf)
            printf '%s\n' "Generic Debian/Kali option:" >&2
            printf '%s\n' "  sudo apt-get update" >&2
            printf '%s\n' "  sudo apt-get install gcc-arm-linux-gnueabihf libc6-dev-armhf-cross" >&2
            ;;
    esac
    printf '%s\n' "Preferred production option: generate/install the SDK from the exact R-Car BSP image and pass its environment-setup-* file with --sdk-env." >&2
    printf '%s\n' "Discover installed SDKs with: scripts/cross-build.sh --list-sdks" >&2
    exit 1
}

missing_sdk_help() {
    local selected_generation="$1"
    local sdk_env="$2"
    local candidates
    candidates="$(discover_sdk_envs)"
    printf 'error: Gen%s SDK environment does not exist: %s\n' "${selected_generation}" "${sdk_env}" >&2
    if [[ -n "${candidates}" ]]; then
        printf '%s\n' "Installed SDK environment files:" >&2
        while IFS= read -r candidate; do
            printf '  %s\n' "${candidate}" >&2
        done <<<"${candidates}"
    else
        printf '%s\n' "No installed Yocto SDK was found." >&2
        printf '%s\n' "The /opt/rcar-genN paths in examples are placeholders, not bundled SDKs." >&2
        printf '%s\n' "From the matching R-Car BSP build environment, run:" >&2
        printf '%s\n' "  bitbake <image-name> -c populate_sdk" >&2
        printf '%s\n' "  ./tmp/deploy/sdk/<sdk-installer>.sh" >&2
    fi
    exit 1
}

ensure_rust_target() {
    local target="$1"
    local target_libdir
    target_libdir="$(rustc --print target-libdir --target "${target}" 2>/dev/null || true)"
    if [[ -n "${target_libdir}" && -d "${target_libdir}" ]]; then
        return
    fi
    if [[ "${install_target}" == true ]]; then
        command -v rustup >/dev/null 2>&1 || fail "rustup is required by --install-target"
        rustup target add "${target}"
        return
    fi
    fail "Rust target ${target} is not installed; run 'rustup target add ${target}' or pass --install-target"
}

write_linker_wrapper() {
    local wrapper="$1"
    cat >"${wrapper}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
: "${CC:?The sourced SDK must export CC}"
# CC and LDFLAGS are trusted values supplied by the Yocto SDK environment.
# shellcheck disable=SC2086
exec ${CC} "$@" ${LDFLAGS:-}
EOF
    chmod 0755 "${wrapper}"
}

write_config() {
    local selected_generation="$1"
    local destination="$2"
    sed -E \
        -e "s/^generation = [0-9]+$/generation = ${selected_generation}/" \
        -e '/^device_type = /d' \
        -e '/^api_token = /d' \
        "${PROJECT_ROOT}/config/default.toml" >"${destination}"
}

verify_binary_architecture() {
    local target="$1"
    local binary="$2"
    local description
    description="$(file -b "${binary}")"
    case "${target}" in
        aarch64-*) grep -qi 'aarch64\|ARM64' <<<"${description}" || fail "${binary} is not an AArch64 ELF: ${description}" ;;
        armv7-*) grep -qi 'ARM' <<<"${description}" || fail "${binary} is not an ARM ELF: ${description}" ;;
    esac
    printf '%s\n' "${description}"
}

build_generation() (
    set -euo pipefail

    local selected_generation="$1"
    local sdk_env="${sdk_envs[${selected_generation}]}"
    local target="${targets[${selected_generation}]}"
    [[ -n "${generic_sdk_env}" ]] && sdk_env="${generic_sdk_env}"
    [[ -n "${generic_target}" ]] && target="${generic_target}"

    if [[ -n "${sdk_env}" ]]; then
        [[ -f "${sdk_env}" ]] || missing_sdk_help "${selected_generation}" "${sdk_env}"
        # Yocto SDK environment scripts intentionally modify this subshell.
        # shellcheck disable=SC1090
        source "${sdk_env}"
    fi

    if [[ -z "${target}" ]]; then
        if [[ -n "${CC:-}" ]]; then
            target="$(infer_target "${CC}" || true)"
        else
            target="aarch64-unknown-linux-gnu"
        fi
    fi
    [[ -n "${target}" ]] || fail "cannot infer the Rust target for Gen${selected_generation}; pass --gen${selected_generation}-target"

    if [[ "${profile}" == release && -z "${sdk_env}" && "${target}" == *-linux-gnu* && "${allow_generic}" != true ]]; then
        fail "Gen${selected_generation} GNU release builds require its matching Yocto SDK; pass --sdk-env or use --allow-generic for a development-only bundle"
    fi

    if [[ -z "${CC:-}" ]]; then
        CC="$(default_compiler "${target}" || true)"
        [[ -n "${CC}" ]] || fail "no default compiler mapping for ${target}; source an SDK with --sdk-env"
        export CC
    fi

    local compiler_command="${CC%% *}"
    if [[ "${dry_run}" != true ]]; then
        ensure_rust_target "${target}"
        command -v "${compiler_command}" >/dev/null 2>&1 || missing_linker_help "${target}" "${compiler_command}"
    fi

    local target_dir="${PROJECT_ROOT}/target/cross/gen${selected_generation}"
    local bundle_dir="${output_dir}/gen${selected_generation}"
    local wrapper="${target_dir}/sdk-linker"
    local cargo_profile_flag="--release"
    local cargo_profile_dir="release"
    if [[ "${profile}" == debug ]]; then
        cargo_profile_flag=""
        cargo_profile_dir="debug"
    fi

    printf 'Gen%s: target=%s sdk=%s compiler=%s\n' \
        "${selected_generation}" "${target}" "${sdk_env:-<none>}" "${CC}"
    if [[ "${dry_run}" == true ]]; then
        return
    fi

    mkdir -p "${target_dir}" "${bundle_dir}"
    write_linker_wrapper "${wrapper}"

    local cargo_target_key="CARGO_TARGET_${target^^}_LINKER"
    cargo_target_key="${cargo_target_key//-/_}"
    export "${cargo_target_key}=${wrapper}"
    export "CC_${target//-/_}=${wrapper}"
    if [[ -n "${AR:-}" ]]; then
        export "AR_${target//-/_}=${AR%% *}"
    fi

    local cargo_args=(build --locked --target "${target}" --target-dir "${target_dir}")
    [[ -n "${cargo_profile_flag}" ]] && cargo_args+=("${cargo_profile_flag}")
    [[ "${offline}" == true ]] && cargo_args+=(--offline)
    cargo "${cargo_args[@]}"

    local binary="${target_dir}/${target}/${cargo_profile_dir}/edgeagent-rs"
    [[ -x "${binary}" ]] || fail "Cargo did not produce ${binary}"
    install -m 0755 "${binary}" "${bundle_dir}/edgeagent-rs"
    install -m 0644 "${PROJECT_ROOT}/packaging/systemd/edgeagent-rs.service" "${bundle_dir}/edgeagent-rs.service"
    write_config "${selected_generation}" "${bundle_dir}/config.toml"

    local architecture
    architecture="$(verify_binary_architecture "${target}" "${bundle_dir}/edgeagent-rs")"
    local linkage="dynamic"
    command -v readelf >/dev/null 2>&1 || fail "readelf is required to audit binary compatibility"
    if ! readelf --program-headers "${bundle_dir}/edgeagent-rs" | grep -q ' INTERP '; then
        linkage="static"
    fi
    if [[ "${target}" == *-linux-musl* && "${linkage}" != static ]]; then
        fail "${binary} targets musl but is dynamically linked; refusing a non-portable release bundle"
    fi
    local glibc_requirements="none"
    if [[ "${target}" == *-linux-gnu* ]]; then
        glibc_requirements="$(
            readelf --version-info "${bundle_dir}/edgeagent-rs" \
                | grep -o 'GLIBC_[0-9.]*' \
                | sort -Vu \
                | paste -sd, - || true
        )"
        [[ -n "${glibc_requirements}" ]] || glibc_requirements="none"
    fi
    cat >"${bundle_dir}/build-manifest.txt" <<EOF
edgeagent_version=$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"version":"\([^"]*\)".*/\1/p' | head -n1)
generation=${selected_generation}
rust_target=${target}
profile=${profile}
sdk_environment=${sdk_env:-none}
sdk_sysroot=${SDKTARGETSYSROOT:-none}
compiler=${CC}
rustc=$(rustc --version)
elf=${architecture}
linkage=${linkage}
glibc_requirements=${glibc_requirements}
EOF
    (
        cd "${bundle_dir}"
        sha256sum edgeagent-rs config.toml edgeagent-rs.service build-manifest.txt >SHA256SUMS
    )
    printf 'Bundle: %s\n' "${bundle_dir}"
)

mkdir -p "${output_dir}"
for selected_generation in "${generations[@]}"; do
    build_generation "${selected_generation}"
done