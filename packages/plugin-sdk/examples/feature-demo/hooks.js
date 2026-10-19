// 一次性 hook（每个事件独立运行，与常驻 main 并存）：写插件日志 + 工作区文件。
globalThis.onStart = async (ev) => {
  flux.logger.info("[hook] onStart", ev.taskId, ev.url);
};

globalThis.onDone = async (ev) => {
  flux.logger.info("[hook] onDone", ev.taskId, ev.filePath);
  await flux.fs.writeFile("last-done.txt", `${ev.taskId} ${ev.filePath}`);
};

globalThis.onError = async (ev) => {
  flux.logger.warn("[hook] onError", ev.taskId, ev.message);
};
