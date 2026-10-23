/**
 * @mediaflow/ui-vue 公开入口。主题 token 子集先于组件样式进入构建产物（dist/style.css 内
 * theme 在前），保证宿主覆盖与依赖顺序稳定。
 */
import "./styles/theme.css";

import MfMenu from "./components/MfMenu.vue";
import MfField from "./components/MfField.vue";
import MfDialog from "./components/MfDialog.vue";
import MfButton from "./components/MfButton.vue";
import MfImage from "./components/MfImage.vue";
import MfContentCard from "./components/MfContentCard.vue";
import MfRequestState from "./components/MfRequestState.vue";
import type { MfButtonType, MfButtonVariant } from "./types";

export type { MfButtonType, MfButtonVariant };
/** MfButton 的 props 类型（由组件实例推导）。 */
export type MfButtonProps = InstanceType<typeof MfButton>["$props"];

/** B1 图片、内容卡和请求容器的公开 props。 */
export type MfImageProps = InstanceType<typeof MfImage>["$props"];
export type MfContentCardProps = InstanceType<typeof MfContentCard>["$props"];
export type MfRequestStateProps = InstanceType<typeof MfRequestState>["$props"];
export type { MfRequestStateValue } from "./types";
export type MfFieldProps = InstanceType<typeof MfField>["$props"];
export type MfDialogProps = InstanceType<typeof MfDialog>["$props"];
export type MfMenuProps = InstanceType<typeof MfMenu>["$props"];
export { MfMenu, MfField, MfDialog, MfButton, MfImage, MfContentCard, MfRequestState };
