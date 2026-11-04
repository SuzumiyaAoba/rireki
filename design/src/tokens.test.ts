// styles.css の :root の CSS 変数が tokens.ts の値と一致することを検査する（Node 環境で動く単体のテスト）。
//
// 対象（:root にあり tokens.ts に対応する値があるもの）:
//   - 色: 面・文字・操作と選択・状態・読み込み元・罫線（colors）
//   - 量の青の段階（heat）、ロゴの地色（brand）、幕（overlay の scrim）
//   - 影（--shadow）: 色は tokens.ts の overlay.shadow と完全一致。幅（0 8px 30px）は tokens.ts にないため、このテストに固定して書く
//   - 角丸（radius の sm・md・panel・inner）、文字サイズ（fontSize の 5 段）、透明度（alpha の 4 段）
//   - 透明度を混ぜて作る派生色（--accent-range、--diff-add-bg、--diff-del-bg）の式
//
// 対象外:
//   - 余白（space）と骨格の寸法（dimensions）: styles.css の :root には変数がなく、
//     クラスの値として直接書いている。寸法の一致は theme::tests（Rust 側）が検査する。
//   - 角丸の full（9999px）: 点と目印の値で、:root に変数を置いていない。
//   - Storybook 専用の変数: --sidebar-w・--detail-w（幅）、--split（統計の割合）、
//     --heat（月の日の帯）、--event-color・--project-color・--source-color（要素ごとの色）は
//     React が inline の style で設定するため、:root には無い。
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { alpha, brand, colors, fontSize, heat, overlay, radius } from './tokens';

const css = readFileSync(fileURLToPath(new URL('./styles.css', import.meta.url)), 'utf8');

// :root の宣言を名前と値の対応にする（コメントは外す）
const rootBlock = /:root\s*\{([\s\S]*?)\n\}/.exec(css.replace(/\/\*[\s\S]*?\*\//g, ''))?.[1] ?? '';
const vars = new Map<string, string>();
for (const match of rootBlock.matchAll(/--([a-z0-9-]+):\s*([^;]+);/g)) {
  vars.set(match[1], match[2].trim().toLowerCase());
}

// tokens.ts の camelCase の名前を CSS の kebab-case に直す（accentBg → accent-bg）
const kebab = (name: string) => name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);

const cssVar = (name: string) => {
  const value = vars.get(name);
  if (value === undefined) throw new Error(`styles.css の :root に --${name} がない`);
  return value;
};

describe('styles.css の :root と tokens.ts の一致', () => {
  it('色（colors）は同じ値である', () => {
    for (const [key, value] of Object.entries(colors)) {
      expect(cssVar(kebab(key)), `--${kebab(key)}`).toBe(value.toLowerCase());
    }
  });

  it('量の青の段階（heat）は --heat-1 から --heat-5 まで同じ値である', () => {
    heat.forEach((value, i) => {
      expect(cssVar(`heat-${i + 1}`), `--heat-${i + 1}`).toBe(value.toLowerCase());
    });
  });

  it('ロゴの地色（brand）と幕・影（overlay）は同じ値である', () => {
    expect(cssVar('logo-claude')).toBe(brand.claude.toLowerCase());
    expect(cssVar('logo-codex')).toBe(brand.codex.toLowerCase());
    expect(cssVar('scrim')).toBe(overlay.scrim.toLowerCase());
    // tokens.ts には影の色（overlay.shadow）だけがあり、幅（0 8px 30px）は styles.css にしかない。
    // 幅はここで固定し、色は tokens.ts と完全一致で比べる
    expect(cssVar('shadow')).toBe(`0 8px 30px ${overlay.shadow.toLowerCase()}`);
  });

  it('角丸（radius）の 4 段は px の値である（full を除く）', () => {
    for (const key of ['sm', 'md', 'panel', 'inner'] as const) {
      expect(cssVar(`radius-${key}`), `--radius-${key}`).toBe(`${radius[key]}px`);
    }
  });

  it('文字サイズ（fontSize）の 5 段は px の値である', () => {
    for (const [key, size] of Object.entries(fontSize)) {
      expect(cssVar(`font-${key}`), `--font-${key}`).toBe(`${size}px`);
    }
  });

  it('透明度（alpha）の 4 段は同じ数値である', () => {
    for (const [key, value] of Object.entries(alpha)) {
      expect(cssVar(`alpha-${key}`), `--alpha-${key}`).toBe(String(value));
    }
  });

  it('透明度を混ぜた派生色は、対応する色と透明度の変数から作られている', () => {
    // 式の形は styles.css の書き方と同じにそろえて比べる
    const mix = (color: string, a: string) =>
      `color-mix(in srgb, var(--${color}) calc(var(--alpha-${a}) * 100%), transparent)`;
    expect(cssVar('accent-range')).toBe(mix('accent', 'range'));
    expect(cssVar('diff-add-bg')).toBe(mix('diff-add', 'diff'));
    expect(cssVar('diff-del-bg')).toBe(mix('diff-del', 'diff'));
  });
});
