# Compatible native Kev server patch

This optional source patch series adds mapped loading, bounded Vulkan Q8_0 streaming,
adaptive memory profiles, idle unload and cooperative vision/UI reservations to
[monatis/ggmlc](https://github.com/monatis/ggmlc) revision
`5490a13661d95add025ec1b111adbc88234a9b4f` (v0.9.7).
It is not a replacement model or a bundled upstream binary. Kritic remains usable
without this patch; unsupported servers simply skip memory reservations.

In a clean checkout at that revision, apply all four patches before configuring/building:

```sh
git checkout 5490a13661d95add025ec1b111adbc88234a9b4f
git submodule update --init --recursive
git apply --check /path/to/native-kev/patches/*.patch
git apply /path/to/native-kev/patches/*.patch
cmake -S . -B build -DGGMLC_BUILD_TESTS=ON -DGGMLC_ENABLE_VULKAN=ON -DGGMLC_ENABLE_CUDA=OFF
cmake --build build --target laya test-loader test-loader-heap-benchmark test-executor-cpu test-memory-policy test-memory-reservations test-weight-streaming-eligibility
ctest --test-dir build --output-on-failure
```

Use a supported C++/CMake toolchain and Vulkan SDK for your platform. On the tested
Windows setup this used MSVC and Ninja; add `-G Ninja` in an initialized MSVC shell.
Upstream declares MIT in its package metadata and README, but the referenced
top-level LICENSE file is absent at the pinned revision. This release bundles
only source patches, with no native server binary or model weights. The patches
include source/tests, not build outputs or machine-specific launcher configuration.

Start the resulting server with your GGUF and appropriate device mapping:

```sh
laya serve /path/to/model.gguf --device vulkan:0 --max-batch 1 --memory-profile auto --idle-seconds 60
```

See [memory profiles and protocol](../../docs/model-memory.md) for overrides,
starting thresholds, lease semantics and limits. GPU ordinal 0 is an example;
verify the physical device rather than copying another machine's GPU ordinal.
Only the stated Q8_0/RTX 2070 workload has measured live equivalence evidence.
Native unit tests also exercise CPU and unsupported-model eligibility boundaries;
these do not establish Mac/Metal, CUDA or arbitrary-model performance parity.

The series groups mapped loading, bounded streaming, adaptive policy, and server
integration separately. Apply the complete ordered series before building; these
are dependent implementation layers.
