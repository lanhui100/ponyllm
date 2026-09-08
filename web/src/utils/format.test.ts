import { describe, it, expect } from 'vitest';
import { formatStrategyLabel, formatTierLabel, formatKeyState } from './format';

describe('format utility', () => {
  it('formats strategies into colloquial Chinese', () => {
    expect(formatStrategyLabel('round_robin')).toBe('轮询');
    expect(formatStrategyLabel('ROUND_ROBIN')).toBe('轮询');
    expect(formatStrategyLabel('economy')).toBe('经济优先');
    expect(formatStrategyLabel('speed')).toBe('速度优先');
    expect(formatStrategyLabel('reliable')).toBe('稳定优先');
    expect(formatStrategyLabel('balanced')).toBe('综合均衡');
    expect(formatStrategyLabel('priority')).toBe('优先级');
    expect(formatStrategyLabel('weighted_round_robin')).toBe('加权轮询');
    expect(formatStrategyLabel('custom')).toBe('custom');
    expect(formatStrategyLabel('')).toBe('未配置');
  });

  it('formats model tier into colloquial Chinese', () => {
    expect(formatTierLabel('Smart')).toBe('主力');
    expect(formatTierLabel('Standard')).toBe('主力');
    expect(formatTierLabel('Large')).toBe('旗舰');
    expect(formatTierLabel('Flagship')).toBe('旗舰');
    expect(formatTierLabel('Fast')).toBe('轻量');
    expect(formatTierLabel('Light')).toBe('轻量');
    expect(formatTierLabel('Fallback')).toBe('兜底');
    expect(formatTierLabel('CustomTier')).toBe('CustomTier');
  });

  it('formats key state into Chinese', () => {
    expect(formatKeyState('active')).toBe('就绪');
    expect(formatKeyState('cooling_down')).toBe('冷却中');
    expect(formatKeyState('disabled')).toBe('已禁用');
  });
});
