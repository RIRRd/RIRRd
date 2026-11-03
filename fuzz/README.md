# Parser fuzzing

Run the RPSL target from the repository root on a Linux host with a native C++
compiler, Rust nightly and cargo-fuzz.

```sh
rustup toolchain install nightly
cargo install cargo-fuzz --locked
cargo +nightly fuzz run rpsl fuzz/seeds/rpsl -- -max_total_time=60 -max_len=65537
```

The target tests the production parser on arbitrary UTF-8 input. Every accepted
object must preserve its original text, have uppercase ASCII identity fields
and reparse to the same identity after public rendering. Rendering a second
time must produce identical text. The snapshot reader validates UTF-8 before
calling the parser, which accepts Rust strings.

Pass a writable copy of the seed directory to libFuzzer when you want to save
coverage-increasing inputs without modifying the repository. Preserve any crash
input and reproduce it by passing its path after the target name.

Stable Rust unit tests separately generate alternate IPv6 route spellings, CIDRs
with invalid host bits, and authentication continuations using LF and CRLF.

The initial Windows GNU check type-checked the target against the locked Rust
dependencies. Linking and running libFuzzer require a host with a C++ toolchain
and cargo-fuzz. The initial Windows host lacked these tools and nightly Rust.
