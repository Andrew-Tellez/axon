# Install

One binary. No runtime, no Node, no Python, no JVM. It runs the same on your laptop and
in an empty CI container.

```sh
curl -fsSL https://raw.githubusercontent.com/Andrew-Tellez/axon/main/install.sh | sh
```

macOS and Linux, `arm64` and `x86_64`. The installer accepts two variables:

| | |
| --- | --- |
| `AXON_INSTALL_DIR` | Where to put the binary. `~/.local/bin` by default |
| `AXON_VERSION` | A specific version, for example `v0.1.0`. The latest by default |

From source, if you would rather build it:

```sh
cargo install --git https://github.com/Andrew-Tellez/axon
```

## For the whole flow

`axon` on its own needs nothing. These tools are needed for what it **generates**, and
each one only for its own part:

| | What for |
| --- | --- |
| Docker | `axon infra --target local` and the system on your machine |
| Terraform | applying what comes out of `--target gcp` or `--target aws` |
| `kubectl` | applying what comes out of `--target k8s` |
| Node 24+ | running the testkit from `axon test`, with no build step |
| k6 | the load tests from `axon load` |
| Flyway or similar | applying the migrations; axon reads them, it does not run them |
| pgdog | `axon pooler`, if you declare a pooler or sharding |

None of them is mandatory: if one is missing, what gets skipped is that part, not the
rest. The complete list, and **how what each one generates is verified**, is in the
[README](https://github.com/Andrew-Tellez/axon#what-it-is-built-with-and-what-verifies-it).
