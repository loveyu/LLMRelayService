// 根 ESLint 配置，覆盖后端 src/（console 有自己的 eslint.config.js）。
// 关键动机: tsc 对「类型层合法、但运行时未 import 的符号」会漏报 —— 例如 drizzle-orm 的
// ne 被某 d.ts 全局化后, tsc 认为 ne 合法不报 "Cannot find name", 但代码没 import ne,
// 运行时 ReferenceError(曾导致 /#/providers 500)。这里用 no-undef 兜底:
// @typescript-eslint/parser 只对 value 引用报警（不误报 type/interface/enum）, 能抓这类漏网符号。
import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import globals from 'globals';

export default tseslint.config(
  {
    ignores: ['dist', 'node_modules', 'console', 'test', 'scripts', '**/*.d.ts'],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ['src/**/*.ts'],
    languageOptions: {
      ecmaVersion: 2021,
      sourceType: 'module',
      globals: {
        ...globals.node,
        Bun: 'readonly',
        // 下面两个是 TS 全局类型（NodeJS namespace / DOM 的 BodyInit），eslint no-undef 会当 value 误报,
        // 声明 readonly 让其跳过（它们由 @types/node 与 tsconfig 的 dom lib 提供, 运行时不会作 value 用）。
        NodeJS: 'readonly',
        BodyInit: 'readonly',
      },
    },
    rules: {
      // 核心: 抓「类型层合法但运行时未 import」的漏网符号（ne 类 bug, tsc 漏报）。
      'no-undef': 'error',
      // 历史代码暂未全面合规, 先关掉这几个高频规则; 后续逐步清理后再开启。
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-require-imports': 'off',
      '@typescript-eslint/no-unused-vars': 'off',
      'no-empty': 'off',
      'no-empty-function': 'off',
    },
  },
);
