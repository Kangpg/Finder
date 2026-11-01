---
name: finder-development
description: Implement and diagnose Finder, the Rust Windows x64 desktop search project, including rendering, scrolling, visible-row metadata, and release replacement. Use for this app's source work only.
---

# Finder development

The product is Finder, published by Kangpg. Read Cargo.toml and product.json to identify the project. Verify its actual source, Cargo configuration, and local instructions before editing. Where filesystem permissions require staging, keep edits in a writable location and use hash-checked synchronization so foreign changes are preserved; a stale staging copy is not authority to overwrite the project.

## Diagnose the actual phase

Distinguish query completion, result publication, scrolling/selection of already published rows, and native window focus/presentation. A completed 27-row query that freezes other apps on focus needs graphics/presentation evidence, not another search-index optimization by default. Verify that focus-dependent CPU/GPU samples actually include the target foreground process; inactive samples do not prove the active case. State separately what source inspection, headless measurements, native device checks, and user observations establish.

The app uses egui/eframe. Read the installed pinned crate source when changing its input, texture-delta, or renderer contracts. Current desktop output selection is owned by src/graphics.rs; --renderer direct3d selects Windows DX12 and --renderer opengl retains a comparison path. A successful headless device or egui mesh test does not establish desktop presentation, compositor behavior, or resolution of a focus-triggered freeze. Do not change system graphics/Parsec/driver settings merely to change this app's renderer.

For development-history and issue reports, reconcile dated VERIFICATION.md entries with current source and give each unresolved symptom an explicit closure criterion. Mark report-created issue IDs as local records rather than implying an external tracker exists. Verify Everything's official stable/beta documentation when comparing features: Everything also has x64 builds, so bitness alone is not a Finder advantage. Keep older measurements tied to their original workload and build rather than presenting them as current-release benchmarks.

## Keep row resources and freshness separate

Search and index work use immutable snapshots and background lanes. Visible-row enrichment is independent of searches; avoid file/Shell I/O in row painting and avoid fetching display metadata for all returned rows before publishing results. Sorting may need metadata for all matching records to remain correct.

The row worker owns TTL/revision freshness. Scrolling out of view does not invalidate a cached icon texture. Keep a bounded display cache, protect visible entries when evicting old entries, and replace/remove textures when the worker reports a new icon or failure. Preserve icon overlays as Shell-owned information; image/overlay indices are not SVN status semantics. Verify cache saturation, returning to previous rows, and refresh failure/replacement, as well as upload counts.

Use real egui input and production worker interfaces for scrolling tests. Explicitly clear intentionally unuploaded TexturesDelta values in headless consumers after observing them. Measure completed, equivalent workloads and label GPU presentation as unmeasured when no native surface was presented. Report regressions and resource tradeoffs instead of equating fewer texture uploads with faster UI frames.

## Validate and deliver

Use the project's configured MSVC x64 target, cargo fmt --check, relevant cargo test --locked checks, and cargo clippy --locked --all-targets -- -D warnings. Opt-in gui::tests::focused_wheel_measurement exercises 27 temporary XML rows with actual wheel events/native icon workers. Opt-in graphics::tests::real_direct3d_device_and_egui_pipeline checks a real DX12 device and offscreen submission without a visible window. These tests create their own resources; they are not permission to interact with the user's desktop.

Release GUI and CLI binaries are distinct PE subsystems (2 and 3). Preserve old executables before replacement, verify x64/PE subsystem, hidden --version completion, and any archive contents. Do not kill or relaunch the user's app to complete packaging unless authorized. A mapped old executable can keep running after its path is replaced; tell the user when full exit/reopen is necessary. Keep settings, indexes, and user files out of distribution archives. Avoid GitKraken hook/integration tools for this app's ordinary source/build work.

## Packaging and source export

Product display name, publisher, identity and installation marker live in product.json and are consumed by build.rs and the release script. Icon geometry is owned by src/icon_art.rs; PE resources, installer, window and tray derive from it. The MSVC target statically links CRT; verify direct PE imports rather than silently installing a system runtime.

Use scripts/build-release.ps1 with NSIS 3.13 to make the per-user setup EXE and portable ZIP. The installer payload alone contains finder.install. app_paths selects profile data for an owned install marker and executable-relative data for portable mode. Unknown or unreadable markers must not silently pick a new data location. Uninstall removes only named product files and its owned integration; profile/portable user data must survive. Verify install and removal in a new temporary directory only after ensuring no existing uninstall registration or shortcut would be overwritten. Do not start the GUI as part of silent packaging verification. NSIS uses a 32-bit bootstrapper around the x64 application; distinguish the two PE architectures in reports.

Use scripts/export-source.ps1 to copy an allowlist to a new external folder. Include Cargo.lock, product.json and installation/build sources. Exclude executables, target, artifacts, user data/indexes, .git and local VERIFICATION.md measurements. Validate copied hashes and reject reparse entries or existing destination contents. A source copy is not authorization to initialize, commit or push a Git repository.
