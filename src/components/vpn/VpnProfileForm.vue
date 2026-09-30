<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { VpnMode, VpnProfile } from '@/types/vpn'
import { MODE_DESCRIPTIONS, MODE_LABELS } from '@/types/vpn'

const props = defineProps<{
  profile: VpnProfile
  busy: boolean
}>()

const emit = defineEmits<{
  update: [patch: Partial<VpnProfile>]
  'save-password': [plain: string]
  remove: []
}>()

const passwordInput = ref(props.profile.password)
const showPassword = ref(false)
const showAdvanced = ref(false)
const passwordSaved = ref(false)

// 切换配置时把该配置已保存的密码回填到输入框（默认遮蔽显示）
watch(
  () => props.profile.id,
  () => {
    passwordInput.value = props.profile.password
    passwordSaved.value = false
    showPassword.value = false
  },
)

// 与已保存密码保持同步（保存成功、外部刷新后都回到真实值）
watch(
  () => props.profile.password,
  (value) => {
    passwordInput.value = value
  },
)

const hasPassword = computed(() => !!props.profile.password)

function savePassword() {
  const plain = passwordInput.value
  if (!plain) return
  emit('save-password', plain)
  passwordSaved.value = true
  setTimeout(() => (passwordSaved.value = false), 2500)
}

const extraArgsText = computed({
  get: () => (props.profile.extra_args ?? []).join('\n'),
  set: (value: string) =>
    emit('update', {
      extra_args: value
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean),
    }),
})

const PROTOCOLS = ['anyconnect', 'nc', 'gp', 'pulse', 'f5', 'fortinet', 'array']
</script>

<template>
  <div class="profile-form">
    <div class="form-grid">
      <label class="field">
        <span class="field-label">配置名称</span>
        <input
          class="form-input"
          :value="profile.name"
          placeholder="例如 公司 VPN"
          @change="emit('update', { name: ($event.target as HTMLInputElement).value })"
        />
      </label>

      <label class="field">
        <span class="field-label">服务器地址</span>
        <input
          class="form-input mono"
          :value="profile.server"
          placeholder="vpn.example.com:1218"
          spellcheck="false"
          @change="emit('update', { server: ($event.target as HTMLInputElement).value.trim() })"
        />
      </label>

      <label class="field">
        <span class="field-label">用户名</span>
        <input
          class="form-input"
          :value="profile.username"
          spellcheck="false"
          @change="emit('update', { username: ($event.target as HTMLInputElement).value.trim() })"
        />
      </label>

      <div class="field">
        <span class="field-label">
          密码
          <em v-if="hasPassword" class="saved-tag">已加密保存</em>
        </span>
        <div class="password-row">
          <input
            class="form-input"
            :type="showPassword ? 'text' : 'password'"
            v-model="passwordInput"
            placeholder="填写后点右侧保存"
            spellcheck="false"
            @keydown.enter="savePassword"
          />
          <button class="btn btn-secondary btn-xs" @click="showPassword = !showPassword">
            {{ showPassword ? '隐藏' : '显示' }}
          </button>
          <button
            class="btn btn-primary btn-xs"
            :disabled="!passwordInput"
            @click="savePassword"
          >
            保存
          </button>
        </div>
        <span v-if="passwordSaved" class="field-ok">密码已加密写入本地配置</span>
      </div>
    </div>

    <div class="section">
      <div class="section-label">分流模式</div>
      <div class="mode-row">
        <button
          v-for="mode in (['customer_first', 'company_first'] as VpnMode[])"
          :key="mode"
          class="mode-card"
          :class="{ 'mode-card-active': profile.mode === mode }"
          @click="emit('update', mode === 'company_first' ? { mode, use_vpn_dns: true } : { mode })"
        >
          <strong>{{ MODE_LABELS[mode] }}</strong>
          <span>{{ MODE_DESCRIPTIONS[mode] }}</span>
        </button>
      </div>
      <p class="tips">
        模式随时可以切，但只有断开重连后才会真正生效。
      </p>
    </div>

    <div class="section">
      <div class="form-grid">
        <label class="field">
          <span class="field-label">
            协议
            <em>服务端类型，不确定就保持 anyconnect</em>
          </span>
          <select
            class="form-input"
            :value="profile.protocol"
            @change="emit('update', { protocol: ($event.target as HTMLSelectElement).value })"
          >
            <option v-for="protocol in PROTOCOLS" :key="protocol" :value="protocol">
              {{ protocol }}
            </option>
          </select>
        </label>
      </div>

      <label class="check-row">
        <input
          type="checkbox"
          :checked="profile.mode === 'company_first' || profile.use_vpn_dns"
          :disabled="profile.mode === 'company_first'"
          @change="emit('update', { use_vpn_dns: ($event.target as HTMLInputElement).checked })"
        />
        <span>
          使用服务端下发的 DNS
          <em v-if="profile.mode === 'company_first'">
            「以公司网络为主」模式下默认路由已交给隧道，解析必须走公司 DNS，
            公司内网域名才解析得出来，所以这里固定开启。
          </em>
          <em v-else>
            开启后所有域名解析都优先走公司 DNS（本程序会把 VPN 网卡的跃点数降低）；
            关闭则保持客户环境 DNS 不变，公司内网域名可能解析不了。
          </em>
        </span>
      </label>
    </div>

    <div class="section">
      <button class="advanced-toggle" @click="showAdvanced = !showAdvanced">
        {{ showAdvanced ? '收起高级选项' : '展开高级选项' }}
      </button>

      <div v-if="showAdvanced" class="advanced">
        <label class="field">
          <span class="field-label">
            本配置专用连接器路径
            <em>
              一般不用填 —— 到「设置」页统一指定 openconnect 的启动文件即可。
              这里只在某个配置需要单独用另一个版本时才填，它优先于设置页。
            </em>
          </span>
          <input
            class="form-input mono"
            :value="profile.connector_path"
            placeholder="留空则使用设置页里的路径"
            spellcheck="false"
            @change="emit('update', { connector_path: ($event.target as HTMLInputElement).value.trim() })"
          />
        </label>

        <label class="field">
          <span class="field-label">
            额外参数
            <em>每行一条，直接追加到 openconnect 命令行</em>
          </span>
          <textarea
            class="form-input mono textarea"
            v-model="extraArgsText"
            rows="3"
            placeholder="--servercert&#10;pin-sha256:xxxx"
            spellcheck="false"
          ></textarea>
        </label>
      </div>
    </div>

    <div class="form-footer">
      <button class="btn btn-danger btn-xs" :disabled="busy" @click="emit('remove')">
        删除这个配置
      </button>
    </div>
  </div>
</template>

<style scoped>
.profile-form {
  display: flex;
  flex-direction: column;
  gap: 16px;
}
.form-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px 16px;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-width: 0;
}
.field-inline {
  flex: 1;
}
.field-label {
  font-size: 12px;
  color: var(--text-secondary);
  display: flex;
  align-items: center;
  gap: 6px;
}
.field-label em {
  font-style: normal;
  font-size: 11px;
  color: var(--text-muted);
}
.saved-tag {
  font-style: normal;
  font-size: 11px;
  color: var(--success);
  border: 1px solid rgba(16, 185, 129, 0.4);
  background: rgba(16, 185, 129, 0.08);
  border-radius: 8px;
  padding: 0 6px;
}
.form-input {
  width: 100%;
  padding: 7px 10px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  font-size: 13px;
  background: var(--bg-input);
  color: var(--text-primary);
  outline: none;
}
.form-input:focus {
  border-color: var(--primary);
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.textarea {
  resize: vertical;
  line-height: 1.5;
}
.password-row {
  display: flex;
  gap: 6px;
}
.password-row .form-input {
  flex: 1;
  min-width: 0;
}
.btn-xs {
  padding: 5px 10px;
  font-size: 12px;
  white-space: nowrap;
}
.field-ok {
  font-size: 11px;
  color: var(--success);
}
.section {
  border-top: 1px solid var(--border-color);
  padding-top: 14px;
}
.section-label {
  font-size: 12px;
  color: var(--text-secondary);
  margin-bottom: 8px;
}
.section-row {
  display: flex;
  gap: 16px;
}
.mode-row {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px;
}
.mode-card {
  text-align: left;
  border: 1px solid var(--border-color);
  background: var(--bg-input);
  border-radius: 8px;
  padding: 10px 12px;
  cursor: pointer;
  display: flex;
  flex-direction: column;
  gap: 5px;
}
.mode-card strong {
  font-size: 13px;
  color: var(--text-primary);
}
.mode-card span {
  font-size: 12px;
  line-height: 1.55;
  color: var(--text-muted);
}
.mode-card:hover {
  border-color: var(--primary);
}
.mode-card-active {
  border-color: var(--primary);
  background: var(--bg-active);
}
.mode-card-active strong {
  color: var(--text-active);
}
.tips {
  margin-top: 8px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-muted);
}
.tips strong {
  color: var(--text-secondary);
}
.check-row {
  display: flex;
  gap: 8px;
  align-items: flex-start;
  margin-top: 12px;
  font-size: 13px;
  color: var(--text-secondary);
  cursor: pointer;
}
.check-row input {
  margin-top: 2px;
}
.check-row em {
  display: block;
  font-style: normal;
  font-size: 11px;
  color: var(--text-muted);
  line-height: 1.6;
  margin-top: 3px;
}
.advanced-toggle {
  border: none;
  background: transparent;
  color: var(--primary);
  font-size: 12px;
  cursor: pointer;
  padding: 0;
}
.advanced {
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin-top: 12px;
}
.form-footer {
  display: flex;
  justify-content: flex-end;
  border-top: 1px solid var(--border-color);
  padding-top: 12px;
}
</style>
