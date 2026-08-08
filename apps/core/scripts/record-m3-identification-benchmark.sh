#!/bin/sh
# 运行被忽略的 M3 识别容量基准测试，并原子发布一份非空 JSON 产物。
# 入口不接受位置参数；`MEDIAFLOW_M3_BENCH_OUTPUT` 必须指向父目录已存在且尚未创建的绝对路径。
# 脚本会运行 release 基准测试并写入目标产物；缺少必填变量时由 shell 以非零状态终止，
# 无效或已存在的目标路径以 2 退出，Cargo 失败传播其状态，成功测试未生成非空产物时以 1 退出。
set -eu

: "${MEDIAFLOW_M3_BENCH_OUTPUT:?MEDIAFLOW_M3_BENCH_OUTPUT must name the benchmark JSON artifact}"

case "$MEDIAFLOW_M3_BENCH_OUTPUT" in
  /*) ;;
  *)
    printf '%s\n' 'MEDIAFLOW_M3_BENCH_OUTPUT must be an absolute path' >&2
    exit 2
    ;;
esac

output_parent=$(dirname "$MEDIAFLOW_M3_BENCH_OUTPUT")
if [ ! -d "$output_parent" ]; then
  printf '%s\n' 'MEDIAFLOW_M3_BENCH_OUTPUT parent directory must already exist' >&2
  exit 2
fi
if [ -e "$MEDIAFLOW_M3_BENCH_OUTPUT" ]; then
  printf '%s\n' 'MEDIAFLOW_M3_BENCH_OUTPUT must not already exist' >&2
  exit 2
fi

cargo test --release --manifest-path apps/core/Cargo.toml \
  --test identification_capacity \
  m3_identification_host_benchmark_records_real_samples_and_enforces_budgets \
  -- --ignored --exact --nocapture

if [ ! -s "$MEDIAFLOW_M3_BENCH_OUTPUT" ]; then
  printf '%s\n' 'benchmark completed without a non-empty JSON artifact' >&2
  exit 1
fi
