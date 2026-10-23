// ESLint's TS service needs a module boundary for SFC imports; vue-tsc checks actual props.
declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent;
  export default component;
}
