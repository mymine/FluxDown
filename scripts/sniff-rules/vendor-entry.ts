// 规则仓 vendor 入口：打包为 <rules-repo>/vendor/fluxdown-sniff-rules.mjs，
// 让规则仓 CI 与扩展用同一份校验与求值实现。
export { BUILTIN_PACK, compileRules } from "../../fluxDown/rules/engine";
export { PackRejectedError, validatePack } from "../../fluxDown/rules/validate";
export { RULE_ENGINE_VERSION } from "../../fluxDown/rules/schema";
export { hostMatchesPattern, parseHostPattern } from "../../fluxDown/rules/glob";
