// resolver：把 https://fluxdown-demo.invalid/* 改写为一个真实可下载的 1 MB 测试文件。
globalThis.resolve = async (ctx) => {
  flux.logger.info("演示 resolver 命中", ctx.url);
  return {
    url: "https://proof.ovh.net/files/1Mb.dat",
    fileName: "feature-demo-resolved.dat",
  };
};
