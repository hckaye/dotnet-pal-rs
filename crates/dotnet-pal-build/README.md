# dotnet-pal-build

Build-only helper and packaged native .NET adapters. Add as a build dependency. Core C headers are obtained from dotnet-pal-rs; native sources are package-local. Enable `posix` explicitly only for the C POSIX reference providers.

Part of the dotnet-pal-rs workspace. Core contracts live in `dotnet-pal-rs`;
platform implementations are separate dependencies. See the repository
`docs/crate-boundaries.md` for migration and supported qualification profiles.
