//! Build-time constants Vite substitutes into the bundle.
//!
//! `__TOOLOG_VERSION__` is read out of `Cargo.toml` by `vite.config.ts` — see
//! the comment there for why the version has exactly one source. The tests run
//! through Vitest, which applies the same `define`, so this is never undefined.

declare const __TOOLOG_VERSION__: string;
