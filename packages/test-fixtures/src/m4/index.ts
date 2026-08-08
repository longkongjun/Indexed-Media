import downloaderConnectionPage from "../../../../contracts/examples/v1/downloader-connection-page.json" with { type: "json" };
import downloaderConnection from "../../../../contracts/examples/v1/downloader-connection.json" with { type: "json" };
import downloaderConnectionTestResult from "../../../../contracts/examples/v1/downloader-connection-test-result.json" with { type: "json" };
import downloadTaskPage from "../../../../contracts/examples/v1/download-task-page.json" with { type: "json" };
import downloadTask from "../../../../contracts/examples/v1/download-task.json" with { type: "json" };
import downloadTaskChanged from "../../../../contracts/examples/v1/event-download-task-changed.json" with { type: "json" };
import automationSourcePage from "../../../../contracts/examples/v1/automation-source-page.json" with { type: "json" };
import automationSource from "../../../../contracts/examples/v1/automation-source.json" with { type: "json" };
import automationSourceConnectionTestResult from "../../../../contracts/examples/v1/automation-source-connection-test-result.json" with { type: "json" };
import automationWebhookSecretReceipt from "../../../../contracts/examples/v1/automation-webhook-secret-receipt.json" with { type: "json" };
import automationEventPage from "../../../../contracts/examples/v1/automation-event-page.json" with { type: "json" };
import automationEvent from "../../../../contracts/examples/v1/automation-event.json" with { type: "json" };
import identificationEnhancer from "../../../../contracts/examples/v1/identification-enhancer.json" with { type: "json" };
import identificationEnhancerConnectionTestResult from "../../../../contracts/examples/v1/identification-enhancer-connection-test-result.json" with { type: "json" };
import automationSourceChanged from "../../../../contracts/examples/v1/event-automation-source-changed.json" with { type: "json" };
import automationEventChanged from "../../../../contracts/examples/v1/event-automation-event-changed.json" with { type: "json" };
import identificationEnhancerChanged from "../../../../contracts/examples/v1/event-identification-enhancer-changed.json" with { type: "json" };

const restExamples = [
  ["DownloaderConnectionPage", downloaderConnectionPage],
  ["DownloaderConnection", downloaderConnection],
  ["DownloaderConnectionTestResult", downloaderConnectionTestResult],
  ["DownloadTaskPage", downloadTaskPage],
  ["DownloadTask", downloadTask],
  ["AutomationSourcePage", automationSourcePage],
  ["AutomationSource", automationSource],
  ["AutomationSourceConnectionTestResult", automationSourceConnectionTestResult],
  ["WebhookSecretReceipt", automationWebhookSecretReceipt],
  ["AutomationEventPage", automationEventPage],
  ["AutomationEvent", automationEvent],
  ["IdentificationEnhancer", identificationEnhancer],
  ["IdentificationEnhancerConnectionTestResult", identificationEnhancerConnectionTestResult],
] as const;

/** 共享的 M4 下载器连接、任务与最小事件契约示例。 */
export const m4Fixtures = {
  downloaderConnectionPage,
  downloaderConnection,
  downloaderConnectionTestResult,
  downloadTaskPage,
  downloadTask,
  downloadTaskChanged,
  automationSourcePage,
  automationSource,
  automationSourceConnectionTestResult,
  automationWebhookSecretReceipt,
  automationEventPage,
  automationEvent,
  identificationEnhancer,
  identificationEnhancerConnectionTestResult,
  automationSourceChanged,
  automationEventChanged,
  identificationEnhancerChanged,
};

/** 返回下载器管理 Schema 名称与对应正向 REST 示例。 */
export const validM4RestContractExamples = () => restExamples;

/** 返回下载器管理的正向 SSE 示例。 */
export const validM4EventExamples = () => [
  downloadTaskChanged,
  automationSourceChanged,
  automationEventChanged,
  identificationEnhancerChanged,
] as const;
