import bootstrapStatus from "../../../../contracts/examples/v1/identity-bootstrap-status.json" with { type: "json" };
import bootstrapRequest from "../../../../contracts/examples/v1/identity-bootstrap-request.json" with { type: "json" };
import session from "../../../../contracts/examples/v1/identity-session.json" with { type: "json" };
import roots from "../../../../contracts/examples/v1/inbox-root-list.json" with { type: "json" };
import preflight from "../../../../contracts/examples/v1/inbox-preflight.json" with { type: "json" };
import inbox from "../../../../contracts/examples/v1/inbox-directory.json" with { type: "json" };
import task from "../../../../contracts/examples/v1/scan-task.json" with { type: "json" };
import files from "../../../../contracts/examples/v1/scan-files-page.json" with { type: "json" };
import errors from "../../../../contracts/examples/v1/scan-errors-page.json" with { type: "json" };
import progress from "../../../../contracts/examples/v1/event-task-progress.json" with { type: "json" };
import stateChanged from "../../../../contracts/examples/v1/event-task-state-changed.json" with { type: "json" };
import gap from "../../../../contracts/examples/v1/event-stream-gap.json" with { type: "json" };
import invalidSecretLeak from "../../../../contracts/examples/v1/identity-invalid-secret-leak.json" with { type: "json" };

const recovery = {
  taskId: task.id,
  snapshots: {
    retained: { ...task, status: "running", recovering: false },
    recovering: {
      ...task,
      status: "running",
      recovering: true,
      counts: { ...task.counts, visited_directories: 3, observed_files: 4 },
    },
    terminal: {
      ...task,
      status: "partial-success",
      recovering: false,
      counts: { ...task.counts, visited_directories: 4, observed_files: 6 },
    },
  },
  files,
  errors,
} as const;

const contractExamples = [
  bootstrapStatus, bootstrapRequest, session, roots, preflight, inbox, task, files, errors,
  progress, stateChanged, gap,
] as const;

/**
 * 具名共享 M2 REST/SSE 示例，以及包含多个快照的任务恢复场景。
 *
 * 值直接从带版本的契约示例导入；使用者必须将其视为共享夹具，而非可变的应用状态。
 */
export const m2Fixtures = {
  bootstrapStatus, bootstrapRequest, session, roots, preflight, inbox, task, files, errors,
  progress, stateChanged, gap, recovery,
};
/**
 * 返回必须通过对应 v1 Schema 验证的全部正向 REST/SSE 示例。
 *
 * @returns 共享的只读元组；调用方不得修改其中的对象。
 */
export const validContractExamples = () => contractExamples;
/**
 * 返回负向契约测试使用的、刻意构造的身份密钥泄漏反例。
 *
 * @returns 必须始终被拒绝、且绝不能用作应用数据的共享无效夹具。
 */
export const invalidIdentitySecretLeak = () => invalidSecretLeak;
