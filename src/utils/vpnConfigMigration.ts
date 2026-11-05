import type { AppConfig } from '@/types'
import type { VpnPolicy, VpnRuleSet } from '@/types/vpn'

/** 老版本配置里的 VPN 配置可能带 rule_set_id、缺 rules；这里用宽松类型读取 */
type LegacyProfile = Partial<AppConfig['vpn_profiles'][number]> & {
  rule_set_id?: string
}

/**
 * 把老模型（全局 vpn_rule_sets + profile.rule_set_id 引用）迁移成新模型
 * （每个配置自带 rules / rules_default_policy）。
 *
 * - 只读 vpn_rule_sets，不改写、不删除，老数据仍留在 config.json 里可回溯；
 * - 多个配置引用同一份列表时各得一份独立副本；
 * - 幂等：rule_set_id 迁完即删掉，再次调用不会再改任何东西。
 */
export function migrateVpnRuleSets(config: AppConfig): {
  changed: boolean
  orphanLists: number
} {
  const legacy: VpnRuleSet[] = config.vpn_rule_sets ?? []
  const profiles = (config.vpn_profiles ?? []) as LegacyProfile[]
  const referenced = new Set<string>()
  let changed = false

  for (const profile of profiles) {
    const oldId = profile.rule_set_id
    if (oldId !== undefined) {
      const set = legacy.find((item) => item.id === oldId)
      if (set) referenced.add(set.id)
      profile.rules = set ? [...set.rules] : []
      profile.rules_default_policy = (set?.default_policy ?? 'DIRECT') as VpnPolicy
      delete profile.rule_set_id
      changed = true
    }
    if (!Array.isArray(profile.rules)) {
      profile.rules = []
      changed = true
    }
    if (!profile.rules_default_policy) {
      profile.rules_default_policy = 'DIRECT'
      changed = true
    }
  }

  const orphanLists = legacy.filter((set) => !referenced.has(set.id)).length
  return { changed, orphanLists }
}
