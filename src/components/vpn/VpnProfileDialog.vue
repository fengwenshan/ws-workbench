<script setup lang="ts">
import { computed, nextTick, onMounted, reactive, ref, type CSSProperties } from 'vue'
import {
  Form,
  FormItem,
  Input,
  InputPassword,
  Modal,
  Select,
  SelectOption,
  Textarea,
} from 'ant-design-vue'
import type { FormInstance } from 'ant-design-vue'
import type { Rule } from 'ant-design-vue/es/form'
import type { NewVpnProfileInput, VpnProfile } from '@/types/vpn'

const props = defineProps<{
  busy: boolean
  /** 传入则进入编辑模式，不传为新建 */
  profile?: VpnProfile | null
}>()

const emit = defineEmits<{
  close: []
  submit: [input: NewVpnProfileInput]
}>()

const PROTOCOLS = ['anyconnect', 'nc', 'gp', 'pulse', 'f5', 'fortinet', 'array']

const isEdit = computed(() => !!props.profile)
const title = computed(() => (isEdit.value ? '编辑 VPN 配置' : '新建 VPN 配置'))
const passwordPlaceholder = computed(() =>
  isEdit.value ? '留空则保持原密码' : '填写 VPN 密码',
)

/**
 * AntD 的 modal-content 自带 24px 左右内边距，滚动条默认会落在内边距里面。
 * body 向右拉伸 24px、再用 padding 补回来，滚动条就贴住弹窗边框，内容位置不变。
 */
const bodyStyle: CSSProperties = {
  maxHeight: '60vh',
  overflowY: 'auto',
  marginRight: '-24px',
  paddingRight: '24px',
}

const formRef = ref<FormInstance | null>(null)
const nameInputRef = ref<{ focus: () => void } | null>(null)

const formState = reactive({
  name: props.profile?.name ?? '新建 VPN 配置',
  server: props.profile?.server ?? '',
  protocol: props.profile?.protocol ?? 'anyconnect',
  username: props.profile?.username ?? '',
  password: '',
  extraArgs: (props.profile?.extra_args ?? []).join('\n'),
  fakeIpRange: props.profile?.fake_ip_range ?? '198.18.0.0/16',
  fakeIpFilter: (props.profile?.fake_ip_filter ?? []).join('\n'),
})

const rules = computed<Record<string, Rule[]>>(() => ({
  name: [{ required: true, whitespace: true, message: '请填写配置名称', trigger: 'blur' }],
  server: [{ required: true, whitespace: true, message: '请填写服务器地址', trigger: 'blur' }],
  username: [{ required: true, whitespace: true, message: '请填写用户名', trigger: 'blur' }],
  password: isEdit.value
    ? []
    : [{ required: true, whitespace: true, message: '请填写密码', trigger: 'blur' }],
}))

onMounted(() => {
  nextTick(() => nameInputRef.value?.focus())
})

function close() {
  if (!props.busy) emit('close')
}

async function submit() {
  if (props.busy) return
  try {
    await formRef.value?.validate()
  } catch {
    return
  }
  const input: NewVpnProfileInput = {
    name: formState.name.trim(),
    server: formState.server.trim(),
    username: formState.username.trim(),
    protocol: formState.protocol,
    extra_args: formState.extraArgs
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean),
    fake_ip_range: formState.fakeIpRange.trim(),
    fake_ip_filter: formState.fakeIpFilter
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean),
  }
  if (formState.password) input.password = formState.password
  emit('submit', input)
}
</script>

<template>
  <Modal
    :open="true"
    :title="title"
    :width="560"
    centered
    :closable="!busy"
    :keyboard="!busy"
    :mask-closable="false"
    :confirm-loading="busy"
    :ok-text="isEdit ? '保存' : '创建配置'"
    cancel-text="取消"
    :cancel-button-props="{ disabled: busy }"
    :body-style="bodyStyle"
    wrap-class-name="vpn-profile-modal"
    @ok="submit"
    @cancel="close"
  >
    <Form
      ref="formRef"
      :model="formState"
      :rules="rules"
      layout="vertical"
    >
      <FormItem label="配置名称" name="name">
        <Input
          ref="nameInputRef"
          v-model:value="formState.name"
          :maxlength="60"
          placeholder="例如 公司 VPN"
          :disabled="busy"
        />
      </FormItem>

      <FormItem label="服务器地址" name="server">
        <Input
          v-model:value="formState.server"
          placeholder="vpn.example.com:1218"
          spellcheck="false"
          :disabled="busy"
        />
      </FormItem>

      <FormItem label="协议" name="protocol">
        <Select v-model:value="formState.protocol" :disabled="busy">
          <SelectOption v-for="item in PROTOCOLS" :key="item" :value="item">
            {{ item }}
          </SelectOption>
        </Select>
      </FormItem>

      <div class="field-grid">
        <FormItem label="用户名" name="username">
          <Input
            v-model:value="formState.username"
            placeholder="填写 VPN 用户名"
            spellcheck="false"
            :disabled="busy"
          />
        </FormItem>

        <FormItem label="密码" name="password">
          <InputPassword
            v-model:value="formState.password"
            :placeholder="passwordPlaceholder"
            spellcheck="false"
            :disabled="busy"
          />
        </FormItem>
      </div>

      <div class="advanced-section">
        <div class="advanced-title">高级配置</div>

        <FormItem label="额外参数" name="extraArgs">
          <Textarea
            v-model:value="formState.extraArgs"
            :rows="3"
            placeholder="--servercert&#10;pin-sha256:xxxx"
            spellcheck="false"
            :disabled="busy"
          />
        </FormItem>

        <FormItem
          name="fakeIpRange"
          tooltip="一般不用改 —— 仅当内网网段与默认 198.18.0.0/16 冲突时才换一段（例如 198.19.0.0/16）；填错会自动退回默认值。"
        >
          <template #label>fake-ip 地址池</template>
          <Input
            v-model:value="formState.fakeIpRange"
            placeholder="198.18.0.0/16"
            spellcheck="false"
            :disabled="busy"
          />
        </FormItem>

        <FormItem
          name="fakeIpFilter"
          tooltip="这些域名不再伪造假地址，DNS 直接返回真实 IP —— 给打印机、NAS、投屏这类只认真实地址的本地设备用（例如 printer.local）。写进来后这条连接就没有域名了，基于域名的分流规则对它不生效，所以别把要走 VPN 的公司内网站点放进来，那种应该写进「分流规则」。"
        >
          <template #label>不走 fake-ip 的域名</template>
          <Textarea
            v-model:value="formState.fakeIpFilter"
            :rows="3"
            placeholder="*.local&#10;*.corp.example.com"
            spellcheck="false"
            :disabled="busy"
          />
        </FormItem>
      </div>
    </Form>
  </Modal>
</template>

<style scoped>
.field-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  column-gap: 16px;
}
@media (max-width: 520px) {
  .field-grid {
    grid-template-columns: 1fr;
  }
}
.advanced-section {
  border-top: 1px dashed var(--border-color);
  padding-top: 12px;
  margin-top: 4px;
}
.advanced-title {
  font-size: 12px;
  color: var(--text-muted);
  margin-bottom: 12px;
}
</style>
