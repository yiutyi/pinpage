// 功能：装配工具条 WebView 的国际化与界面；磁贴 ID、语言和窗口模式由 Rust 建窗时写入查询参数。
// 一个标签组只有一条工具条，pin 始终是磁贴 ID，而不是某个标签窗口的标签。
import { createApp } from 'vue';
import ToolbarRoot from './features/toolbar/ToolbarRoot.vue';
import { applyLocale, i18n } from './i18n';
import './styles/toolbar.css';

const params = new URLSearchParams(window.location.search);
const locale = params.get('locale') === 'en-US' ? 'en-US' : 'zh-CN';
const mode = params.get('mode') === 'desktop' ? 'desktop' : 'floating';
const slotParam = params.get('slot');
const slot: 'top' | 'left' | 'right' =
  slotParam === 'left' || slotParam === 'right' ? slotParam : 'top';
document.documentElement.dataset.toolbarSlot = slot;
applyLocale(locale);
// 工具条没有系统标题栏，标题只用于无障碍标识，不沿用工作台名称。
document.title = 'PinPage Toolbar';
createApp(ToolbarRoot, {
  pin: params.get('pin') ?? '',
  mode,
  slot,
})
  .use(i18n)
  .mount('#toolbar');
