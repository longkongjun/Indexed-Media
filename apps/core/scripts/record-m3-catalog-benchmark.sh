#!/bin/sh
# 在一次性目录中构造包含 50,000 个条目的确定性 Catalog，并发布一份非空 JSON 证据。
# 入口不接受位置参数；`MEDIAFLOW_BENCHMARK_OUTPUT` 必须指向父目录已存在且尚未创建的绝对路径，
# `TMPDIR` 可以覆盖临时目录的父位置。脚本会创建并删除临时数据库及摘要，同时运行 release fixture 和基准测试。
# 缺少必填变量时由 shell 以非零状态终止；无效或已存在的目标路径以 2 退出，Cargo 失败传播其状态，
# 成功测试未生成非空产物时以 1 退出；HUP、INT、TERM 分别以 129、130、143 退出并清理临时目录。
set -eu

: "${MEDIAFLOW_BENCHMARK_OUTPUT:?MEDIAFLOW_BENCHMARK_OUTPUT must name the benchmark JSON artifact}"

case "$MEDIAFLOW_BENCHMARK_OUTPUT" in
  /*) ;;
  *)
    printf '%s\n' 'MEDIAFLOW_BENCHMARK_OUTPUT must be an absolute path' >&2
    exit 2
    ;;
esac

output_parent=$(dirname "$MEDIAFLOW_BENCHMARK_OUTPUT")
if [ ! -d "$output_parent" ]; then
  printf '%s\n' 'MEDIAFLOW_BENCHMARK_OUTPUT parent directory must already exist' >&2
  exit 2
fi
if [ -e "$MEDIAFLOW_BENCHMARK_OUTPUT" ]; then
  printf '%s\n' 'MEDIAFLOW_BENCHMARK_OUTPUT must not already exist' >&2
  exit 2
fi

benchmark_root=$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-m3-catalog.XXXXXX")
# 保存入口退出状态并删除一次性基准目录；不接收参数、不输出内容，随后恢复原退出状态。
cleanup() {
  status="$?"
  trap - EXIT HUP INT TERM
  rm -rf -- "$benchmark_root"
  exit "$status"
}
# 使用参数 1 提供的信号退出状态删除一次性基准目录；不输出内容，并以该状态结束入口。
interrupted() {
  status="$1"
  trap - EXIT HUP INT TERM
  rm -rf -- "$benchmark_root"
  exit "$status"
}
trap cleanup EXIT
trap 'interrupted 129' HUP
trap 'interrupted 130' INT
trap 'interrupted 143' TERM

database="$benchmark_root/catalog/mediaflow.db"
cargo run --release --quiet --manifest-path apps/core/Cargo.toml \
  --bin m3-catalog-fixture -- --database "$database" --items 50000 --seed 11 \
  > "$benchmark_root/fixture-summary.json"

MEDIAFLOW_CATALOG_BENCH_DATABASE="$database" \
MEDIAFLOW_BENCHMARK_OUTPUT="$MEDIAFLOW_BENCHMARK_OUTPUT" \
cargo test --release --manifest-path apps/core/Cargo.toml \
  --test catalog_capacity \
  m3_catalog_host_benchmark_records_actual_bounds_without_fabricating_a_budget_pass \
  -- --ignored --exact --nocapture

if [ ! -s "$MEDIAFLOW_BENCHMARK_OUTPUT" ]; then
  printf '%s\n' 'benchmark completed without a non-empty JSON artifact' >&2
  exit 1
fi
