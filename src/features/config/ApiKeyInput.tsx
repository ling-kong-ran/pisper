// 单凭据输入直接由表单持有，留空编辑时不覆盖已保存的凭据。
import { useI18n } from '@/app/use-i18n'
import { FieldLabel } from '@/components/ui/field'

type ApiKeyInputProps = {
  value: string
  onChange: (value: string) => void
  configured?: boolean
  cloning?: boolean
}

export function ApiKeyInput({
  value,
  onChange,
  configured = false,
  cloning = false,
}: ApiKeyInputProps) {
  const { t } = useI18n()
  return (
    <FieldLabel variant="control">
      {t('config:configPage.apiKey')}
      <input
        type="password"
        autoComplete="new-password"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={
          cloning
            ? t('config:configPage.leaveBlankToCopySourceKey')
            : configured
              ? t('config:configPage.leaveBlankToKeepExistingKey')
              : t('config:configPage.enterTheProviderAPIKey')
        }
      />
    </FieldLabel>
  )
}
