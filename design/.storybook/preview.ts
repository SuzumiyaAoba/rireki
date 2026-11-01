import type { Preview } from '@storybook/react-vite';
import { themes } from 'storybook/theming';
import { colors } from '../src/tokens';
import '../src/styles.css';

const preview: Preview = {
  parameters: {
    layout: 'fullscreen',
    controls: { expanded: true },
    docs: { theme: themes.dark },
    backgrounds: { options: { dark: { name: 'rireki', value: colors.backdrop } } },
    options: {
      storySort: {
        order: [
          '画面',
          ['タイムライン', 'カレンダー', '統計', '状態'],
          'パネル',
          ['セッション詳細', 'オーバーレイ'],
          'コンポーネント',
          'デザイントークン',
        ],
      },
    },
    viewport: {
      options: {
        native: {
          name: '既定のウィンドウ · 1500 × 920',
          styles: { width: '1500px', height: '920px' },
          type: 'desktop',
        },
        laptop: {
          name: 'ラップトップ · 1280 × 800',
          styles: { width: '1280px', height: '800px' },
          type: 'desktop',
        },
        minimum: {
          name: '最小ウィンドウ · 960 × 600',
          styles: { width: '960px', height: '600px' },
          type: 'desktop',
        },
      },
    },
    // GPUI の配色をそのまま再現するため、コントラスト等はレポートとして確認する。
    a11y: { test: 'todo' },
  },
  initialGlobals: { backgrounds: { value: 'dark' } },
};

export default preview;
