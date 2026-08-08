#!/bin/sh
# 运行被忽略的 Core 扫描容量基准测试，并将非空 JSON 产物写入
# `$MEDIAFLOW_BENCH_OUTPUT`。该变量必须指定一个父目录已存在的绝对路径。
# 未设置变量会在必填变量展开时以非零 shell 状态终止；相对路径或不存在的父目录以 2 退出。
# Cargo 测试失败会原样传播 Cargo 的退出状态；成功测试却未产生产物时以 1 退出。
# 每种失败都会删除可能已生成的不完整产物。
set -eu

: "${MEDIAFLOW_BENCH_OUTPUT:?MEDIAFLOW_BENCH_OUTPUT must name the benchmark JSON artifact}"

case "$MEDIAFLOW_BENCH_OUTPUT" in
  /*) ;;
  *)
    printf '%s\n' 'MEDIAFLOW_BENCH_OUTPUT must be an absolute path' >&2
    exit 2
    ;;
esac

output_parent=$(dirname "$MEDIAFLOW_BENCH_OUTPUT")
if [ ! -d "$output_parent" ]; then
  printf '%s\n' 'MEDIAFLOW_BENCH_OUTPUT parent directory must already exist' >&2
  exit 2
fi

rm -f "$MEDIAFLOW_BENCH_OUTPUT"
benchmark_succeeded=0
# 基准未成功完成时删除目标产物；不接收参数、不输出内容，删除失败会使原退出状态升级为非零。
cleanup_failed_artifact() {
  if [ "$benchmark_succeeded" -ne 1 ]; then
    rm -f "$MEDIAFLOW_BENCH_OUTPUT"
  fi
}
trap cleanup_failed_artifact EXIT HUP INT TERM

cargo test --release --manifest-path apps/core/Cargo.toml \
  --test scan_capacity \
  m2_host_capacity_benchmark_records_real_samples_and_enforces_budgets \
  -- --ignored --exact --nocapture

if [ ! -s "$MEDIAFLOW_BENCH_OUTPUT" ]; then
  printf '%s\n' 'benchmark completed without a non-empty JSON artifact' >&2
  exit 1
fi

benchmark_succeeded=1
