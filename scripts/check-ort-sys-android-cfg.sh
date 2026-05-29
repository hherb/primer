#!/usr/bin/env bash
set -euo pipefail

# Regression guard for the vendored `ort-sys` Android `cache_dir` cfg patch
# (issue #180; protects the fix from PR #179 / issue #157).
#
# WHY THIS EXISTS
# ---------------
# We vendor `ort-sys 2.0.0-rc.10` at src/vendor/ort-sys/ with exactly one
# cfg edit in src/internal/dirs.rs: the Linux `cache_dir()` arm is broadened
# from `#[cfg(target_os = "linux")]` to
# `#[cfg(any(target_os = "linux", target_os = "android"))]`. Without it,
# `ort-sys`'s build.rs (`use ...::dirs::cache_dir;`) fails to resolve (E0432)
# when building NATIVELY on an Android host (Termux, host ==
# aarch64-linux-android), because build scripts compile for the HOST.
#
# No ordinary CI job can catch a regression of this patch:
#   - `cargo build --target aarch64-linux-android` from a Linux runner
#     compiles build scripts for the Linux HOST, so the android arm is never
#     exercised.
#   - `cargo check --features fastembed` runs on the Linux host too.
# So a "modernisation", a dropped cfg arm, or a botched rebase would keep CI
# green while native Termux builds break again.
#
# WHAT THIS DOES
# --------------
# It reproduces PR #179's manual probe at the cfg-resolution level, which
# needs NO Android host and NO NDK linker — only `rustc --target
# aarch64-linux-android --emit=metadata` on a tiny consumer that mimics
# build.rs's `use ...::cache_dir;` import:
#
#   1. GREEN: the vendored (patched) dirs.rs must compile for the android
#      target. A failure here means the patch regressed.
#   2. TEETH: a counterfactual copy with the cfg arm reverted to
#      `#[cfg(target_os = "linux")]` must FAIL with E0432. This proves the
#      test actually exercises the patched arm (a guard that can't fail on
#      the unpatched code is worthless).
#
# Run locally:  scripts/check-ort-sys-android-cfg.sh
# In CI:        invoked from the android-cross-compile job, which already
#               installs the aarch64-linux-android target.

# --- Constants (no magic strings buried inline) ---------------------------
readonly TARGET="aarch64-linux-android"
# The exact cfg line the vendor patch adds. Kept as a literal so a rewording
# of the patch (e.g. switching to target_family) trips the substitution-count
# assertion below rather than silently producing a no-teeth guard.
readonly PATCHED_CFG='#[cfg(any(target_os = "linux", target_os = "android"))]'
readonly UNPATCHED_CFG='#[cfg(target_os = "linux")]'
readonly EXPECTED_ERROR="E0432"
readonly RUSTC="${RUSTC:-rustc}"

# --- Locate the vendored dirs.rs relative to this script ------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
readonly SCRIPT_DIR REPO_ROOT
readonly DIRS_RS="${REPO_ROOT}/src/vendor/ort-sys/src/internal/dirs.rs"

if [[ ! -f "${DIRS_RS}" ]]; then
	echo "FAIL: vendored dirs.rs not found at ${DIRS_RS}" >&2
	echo "      (has the vendored ort-sys crate moved? update this guard.)" >&2
	exit 1
fi

# --- Preconditions: rustc + the android target must be available ----------
if ! command -v "${RUSTC}" >/dev/null 2>&1; then
	echo "FAIL: '${RUSTC}' not found on PATH." >&2
	exit 1
fi
if ! "${RUSTC}" --print target-list 2>/dev/null | grep -qx "${TARGET}"; then
	echo "FAIL: rustc does not know the ${TARGET} target." >&2
	exit 1
fi

# --- Scratch workspace, cleaned up on any exit ----------------------------
WORK_DIR="$(mktemp -d)"
readonly WORK_DIR
trap 'rm -rf "${WORK_DIR}"' EXIT

# Honest precondition: the target's std must be installed, since
# `--emit=metadata` still typechecks against std. Probe with a trivial lib
# rather than `--print sysroot` (which succeeds even when std is absent), so
# a missing target is reported as such instead of as a patch regression.
echo 'pub fn probe() {}' > "${WORK_DIR}/probe.rs"
if ! "${RUSTC}" --target "${TARGET}" --crate-type lib --emit=metadata \
	-o "${WORK_DIR}/probe.rmeta" "${WORK_DIR}/probe.rs" 2>/dev/null; then
	echo "FAIL: the ${TARGET} std component is not installed." >&2
	echo "      Install it with: rustup target add ${TARGET}" >&2
	exit 1
fi

# A consumer module that mimics ort-sys build.rs's import + use of cache_dir.
# `--crate-type lib --emit=metadata` resolves the import without linking, so
# dirs.rs's unresolved `extern "C"` symbols are irrelevant.
write_consumer() {
	local dirs_path="$1" out="$2"
	cat > "${out}" <<RS
#[path = "${dirs_path}"]
mod dirs;
use self::dirs::cache_dir;
pub fn touch() -> Option<std::path::PathBuf> {
	cache_dir()
}
RS
}

compile_consumer() {
	local consumer="$1" stderr="$2"
	"${RUSTC}" --target "${TARGET}" --crate-type lib --emit=metadata \
		-o "${WORK_DIR}/out.rmeta" "${consumer}" 2>"${stderr}"
}

echo "ort-sys android cfg guard: rustc=$("${RUSTC}" --version), target=${TARGET}"

# === Check 1 (GREEN): the patched vendored dirs.rs compiles for android ===
write_consumer "${DIRS_RS}" "${WORK_DIR}/consumer_patched.rs"
if compile_consumer "${WORK_DIR}/consumer_patched.rs" "${WORK_DIR}/patched.err"; then
	echo "PASS: vendored (patched) dirs.rs compiles cache_dir for ${TARGET}."
else
	echo "FAIL: vendored dirs.rs does NOT compile cache_dir for ${TARGET}." >&2
	echo "      The issue #157 / PR #179 android cache_dir cfg patch has regressed." >&2
	echo "      Expected this cfg arm in ${DIRS_RS}:" >&2
	echo "        ${PATCHED_CFG}" >&2
	echo "      ---- rustc output ----" >&2
	cat "${WORK_DIR}/patched.err" >&2
	exit 1
fi

# === Check 2 (TEETH): reverting the cfg arm must reproduce E0432 ==========
# Construct the counterfactual by reverting ONLY the patched cache_dir arm.
# Matching is done with LITERAL (fixed-string) tooling, not sed regex: the
# patched cfg line contains `[](){}"` which a regex would mis-interpret as
# metacharacters.
#
# The match count must be exactly 1: the home_dir() fallback also uses
# `any(target_os = "android", ...)` but with a different operand order, so it
# is not matched. If the count is not 1 the patch has been reworded and this
# guard can no longer build a faithful counterfactual — fail loudly.
subs="$(grep -Fc "${PATCHED_CFG}" "${DIRS_RS}" || true)"
if [[ "${subs}" -ne 1 ]]; then
	echo "FAIL: expected exactly 1 patched cfg line, found ${subs}." >&2
	echo "      The vendor patch line no longer matches:" >&2
	echo "        ${PATCHED_CFG}" >&2
	echo "      Update PATCHED_CFG in this guard to match the new wording." >&2
	exit 1
fi
# Literal, index-based replacement (no regex) of the single patched line.
awk -v old="${PATCHED_CFG}" -v new="${UNPATCHED_CFG}" '
	{
		idx = index($0, old)
		if (idx > 0) {
			$0 = substr($0, 1, idx - 1) new substr($0, idx + length(old))
		}
		print
	}
' "${DIRS_RS}" > "${WORK_DIR}/dirs_unpatched.rs"

write_consumer "${WORK_DIR}/dirs_unpatched.rs" "${WORK_DIR}/consumer_unpatched.rs"
if compile_consumer "${WORK_DIR}/consumer_unpatched.rs" "${WORK_DIR}/unpatched.err"; then
	echo "FAIL: the unpatched counterfactual compiled — the guard has no teeth." >&2
	echo "      cache_dir resolved for ${TARGET} even WITHOUT the android cfg arm," >&2
	echo "      so this guard would not catch a regression. Investigate dirs.rs." >&2
	exit 1
fi
if grep -q "${EXPECTED_ERROR}" "${WORK_DIR}/unpatched.err"; then
	echo "PASS: reverting the cfg arm reproduces ${EXPECTED_ERROR} (guard has teeth)."
else
	echo "FAIL: unpatched copy failed to compile but NOT with ${EXPECTED_ERROR}." >&2
	echo "      The failure mode changed; verify the guard still targets the" >&2
	echo "      right regression before trusting it." >&2
	echo "      ---- rustc output ----" >&2
	cat "${WORK_DIR}/unpatched.err" >&2
	exit 1
fi

echo "OK: ort-sys android cache_dir cfg guard passed."
