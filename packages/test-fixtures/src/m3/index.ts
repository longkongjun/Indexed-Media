import tmdbIntegration from "../../../../contracts/examples/v1/tmdb-integration.json" with { type: "json" };
import tmdbIntegrationRequest from "../../../../contracts/examples/v1/tmdb-integration-request.json" with { type: "json" };
import tmdbConnectionTestRequest from "../../../../contracts/examples/v1/tmdb-integration-connection-test-request.json" with { type: "json" };
import tmdbConnectionTestResult from "../../../../contracts/examples/v1/tmdb-integration-connection-test-result.json" with { type: "json" };
import discoveryPolicy from "../../../../contracts/examples/v1/discovery-policy.json" with { type: "json" };
import discoveryPolicyRequest from "../../../../contracts/examples/v1/discovery-policy-request.json" with { type: "json" };
import processingTask from "../../../../contracts/examples/v1/processing-task.json" with { type: "json" };
import processingTaskPage from "../../../../contracts/examples/v1/processing-task-page.json" with { type: "json" };
import identification from "../../../../contracts/examples/v1/identification-detail.json" with { type: "json" };
import reviewCase from "../../../../contracts/examples/v1/review-case.json" with { type: "json" };
import reviewCasePage from "../../../../contracts/examples/v1/review-case-page.json" with { type: "json" };
import processingStateChanged from "../../../../contracts/examples/v1/event-processing-task-state-changed.json" with { type: "json" };
import identificationDecided from "../../../../contracts/examples/v1/event-processing-task-identification-decided.json" with { type: "json" };
import discoveryHealthChanged from "../../../../contracts/examples/v1/event-inbox-discovery-health-changed.json" with { type: "json" };
import integrationHealthChanged from "../../../../contracts/examples/v1/event-integration-health-changed.json" with { type: "json" };
import reviewDecisionSelectCandidateRequest from "../../../../contracts/examples/v1/review-decision-select-candidate-request.json" with { type: "json" };
import reviewDecisionRematchRequest from "../../../../contracts/examples/v1/review-decision-rematch-request.json" with { type: "json" };
import reviewDecisionGenericVideoRequest from "../../../../contracts/examples/v1/review-decision-generic-video-request.json" with { type: "json" };
import taskDecisionReceipt from "../../../../contracts/examples/v1/task-decision-receipt.json" with { type: "json" };
import reviewCandidatePage from "../../../../contracts/examples/v1/review-candidate-page.json" with { type: "json" };
import mediaItemPage from "../../../../contracts/examples/v1/media-item-page.json" with { type: "json" };
import mediaItemDetail from "../../../../contracts/examples/v1/media-item-detail.json" with { type: "json" };
import taskDecisionAccepted from "../../../../contracts/examples/v1/event-task-decision-accepted.json" with { type: "json" };
import reviewCaseUpdated from "../../../../contracts/examples/v1/event-review-case-updated.json" with { type: "json" };
import catalogMediaChanged from "../../../../contracts/examples/v1/event-catalog-media-changed.json" with { type: "json" };
import organizationTargetPage from "../../../../contracts/examples/v1/organization-target-page.json" with { type: "json" };
import organizationTarget from "../../../../contracts/examples/v1/organization-target.json" with { type: "json" };
import organizationTargetPreflight from "../../../../contracts/examples/v1/organization-target-preflight.json" with { type: "json" };
import processingTaskOrganization from "../../../../contracts/examples/v1/processing-task-organization.json" with { type: "json" };
import organizationTargetChanged from "../../../../contracts/examples/v1/event-organization-target-changed.json" with { type: "json" };
import organizationResultChanged from "../../../../contracts/examples/v1/event-organization-result-changed.json" with { type: "json" };

const restExamples = [
  ["TmdbIntegration", tmdbIntegration],
  ["PutTmdbIntegrationRequest", tmdbIntegrationRequest],
  ["TmdbConnectionTestRequest", tmdbConnectionTestRequest],
  ["TmdbConnectionTestResult", tmdbConnectionTestResult],
  ["DiscoveryPolicy", discoveryPolicy],
  ["PutDiscoveryPolicyRequest", discoveryPolicyRequest],
  ["ProcessingTask", processingTask],
  ["ProcessingTaskPage", processingTaskPage],
  ["IdentificationDetail", identification],
  ["ReviewCase", reviewCase],
  ["ReviewCasePage", reviewCasePage],
  ["SelectProviderCandidateCommand", reviewDecisionSelectCandidateRequest],
  ["RematchWithHintsCommand", reviewDecisionRematchRequest],
  ["SelectGenericVideoCommand", reviewDecisionGenericVideoRequest],
  ["TaskDecisionReceipt", taskDecisionReceipt],
  ["ReviewCandidatePage", reviewCandidatePage],
  ["MediaItemPage", mediaItemPage],
  ["MediaItemDetail", mediaItemDetail],
  ["OrganizationTargetPage", organizationTargetPage],
  ["OrganizationTarget", organizationTarget],
  ["OrganizationTargetPreflight", organizationTargetPreflight],
  ["ProcessingTaskOrganization", processingTaskOrganization],
] as const;

const eventExamples = [
  processingStateChanged,
  identificationDecided,
  discoveryHealthChanged,
  integrationHealthChanged,
  taskDecisionAccepted,
  reviewCaseUpdated,
  catalogMediaChanged,
  organizationTargetChanged,
  organizationResultChanged,
] as const;

/** 共享的 M3 持续发现、识别、审核和健康契约示例。 */
export const m3Fixtures = {
  tmdbIntegration,
  tmdbIntegrationRequest,
  tmdbConnectionTestRequest,
  tmdbConnectionTestResult,
  discoveryPolicy,
  discoveryPolicyRequest,
  processingTask,
  processingTaskPage,
  identification,
  reviewCase,
  reviewCasePage,
  processingStateChanged,
  identificationDecided,
  discoveryHealthChanged,
  integrationHealthChanged,
  reviewDecisionSelectCandidateRequest,
  reviewDecisionRematchRequest,
  reviewDecisionGenericVideoRequest,
  taskDecisionReceipt,
  reviewCandidatePage,
  mediaItemPage,
  mediaItemDetail,
  taskDecisionAccepted,
  reviewCaseUpdated,
  catalogMediaChanged,
  organizationTargetPage,
  organizationTarget,
  organizationTargetPreflight,
  processingTaskOrganization,
  organizationTargetChanged,
  organizationResultChanged,
};

/** 返回 Schema 名称与对应正向 REST 示例的只读元组。 */
export const validM3RestContractExamples = () => restExamples;

/** 返回全部 M3 正向 SSE 示例。 */
export const validM3EventExamples = () => eventExamples;

/** 返回必须被响应 Schema 拒绝的凭据泄漏反例。 */
export const invalidM3ResponseSecretLeak = () => ({
  ...tmdbIntegration,
  api_read_access_token: tmdbIntegrationRequest.api_read_access_token,
});

/** 返回必须被事件 Schema 拒绝的凭据泄漏反例。 */
export const invalidM3EventSecretLeak = () => ({
  ...integrationHealthChanged,
  payload: {
    ...integrationHealthChanged.payload,
    api_read_access_token: tmdbConnectionTestRequest.api_read_access_token,
  },
});
