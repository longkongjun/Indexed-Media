import type { Router } from "vue-router";

const invalidators = new WeakMap<Router, () => void>();

/**
 * 将路由器与清除其缓存初始化判定的回调关联起来。
 *
 * @param router - 导航守卫持有该缓存判定的路由器。
 * @param invalidate - 清除缓存的回调；再次注册会替换之前的回调。
 * @remarks 弱关联不会持有路由器，且本函数不会调用该回调。
 */
export function registerBootstrapInvalidator(router: Router, invalidate: () => void): void {
  invalidators.set(router, invalidate);
}

/**
 * 在已注册回调时，使路由器缓存的初始化判定失效。
 *
 * @param router - 初始化状态已过期的路由器。
 * @remarks 同步调用已注册的回调；未注册时不产生任何效果。
 */
export function invalidateBootstrapState(router: Router): void {
  invalidators.get(router)?.();
}
