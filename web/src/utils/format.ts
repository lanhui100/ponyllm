/**
 * UI 格式化辅助工具函数（策略回表中文映射、分级徽标本土化、状态翻译）
 */

export function formatStrategyLabel(strategy?: string | null): string {
  if (!strategy) return '未配置';
  const s = strategy.toLowerCase().trim();
  switch (s) {
    case 'round_robin':
      return '轮询';
    case 'priority':
      return '优先级';
    case 'weighted_round_robin':
    case 'weighted':
      return '加权轮询';
    case 'economy':
      return '经济优先';
    case 'speed':
      return '速度优先';
    case 'reliable':
      return '稳定优先';
    case 'balanced':
      return '综合均衡';
    default:
      return strategy;
  }
}

export function formatTierLabel(tier?: string | null): string {
  if (!tier) return '主力';
  const t = tier.toLowerCase().trim();
  switch (t) {
    case 'smart':
    case 'standard':
      return '主力';
    case 'large':
    case 'flagship':
      return '旗舰';
    case 'fast':
    case 'light':
      return '轻量';
    case 'fallback':
      return '兜底';
    default:
      return tier;
  }
}

export function formatKeyState(state?: string | null): string {
  if (!state) return '未知';
  const s = state.toLowerCase().trim();
  switch (s) {
    case 'active':
      return '就绪';
    case 'cooling_down':
    case 'coolingdown':
      return '冷却中';
    case 'disabled':
      return '已禁用';
    default:
      return state;
  }
}

/**
 * 统一格式化上下文窗口展示字符串，彻底消除大小写混用（如 128k -> 128K, 1m -> 1M）
 */
export function formatContextWindow(ctx?: string | null): string {
  if (!ctx) return '256K';
  const trimmed = ctx.trim();
  if (!trimmed) return '256K';
  // 匹配类似 128k, 256k, 1m, 200k 等后缀单位并大写转换
  return trimmed.replace(/([0-9]+)\s*([kKmMgGtT])/g, (_, num, unit) => `${num}${unit.toUpperCase()}`);
}

