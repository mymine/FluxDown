// 构建期生成：assets/i18n/*.json 全部语言表，键为小写语言代码（见 web/i18n-tables.ts）。
declare module 'virtual:i18n-tables' {
  const tables: Readonly<Record<string, Readonly<Record<string, string>>>>
  export default tables
}
