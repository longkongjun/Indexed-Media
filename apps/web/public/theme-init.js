// 首帧前恢复主题与皮肤；同源外部脚本遵循Core原有script-src限制。
(function () {
  let saved = null, skin = null;
  try {
    saved = localStorage.getItem("mfd-theme");
    skin = localStorage.getItem("mfd-skin");
  } catch {
    // 隐私模式下保留系统默认值。
  }
  if (saved === "light" || saved === "dark") {
    document.documentElement.dataset.theme = saved;
  }
  if (skin === "aurora" || skin === "sepia") {
    document.documentElement.dataset.skin = skin;
  }
})();
