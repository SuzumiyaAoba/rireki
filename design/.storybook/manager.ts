import { addons } from 'storybook/manager-api';
import { create } from 'storybook/theming';

addons.setConfig({
  theme: create({
    base: 'dark',
    brandTitle: 'rireki · UI',
    colorPrimary: '#5b9cf5',
    colorSecondary: '#5b9cf5',
    appBg: '#0e1014',
    appContentBg: '#16181d',
    appBorderColor: '#2c313b',
    barBg: '#15171c',
    fontBase: '-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
  }),
});
