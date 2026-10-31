// 组件错误码 → 文案（计划 §3.2：协议只给 code + params，由前端按 custom-ui:errors.<code> 本地化）。
// check-i18n 只认 t() 的字面量键，所以逐个写成静态表；表里没有的码原样显示，不伪装成别的错误。
type Translate = (key: string, values?: Record<string, string | number>) => string

export function componentErrorText(
  code: string,
  t: Translate,
  params?: Record<string, string | number>,
): string {
  const max = params?.max ?? ''
  const labels: Record<string, () => string> = {
    // 目录与 manifest（列表行、设置栏的错误清单）
    manifest_missing: () => t('custom-ui:errors.manifest_missing'),
    manifest_too_large: () => t('custom-ui:errors.manifest_too_large'),
    manifest_json_invalid: () => t('custom-ui:errors.manifest_json_invalid'),
    manifest_not_object: () => t('custom-ui:errors.manifest_not_object'),
    entry_missing: () => t('custom-ui:errors.entry_missing'),
    style_missing: () => t('custom-ui:errors.style_missing'),
    component_too_large: () => t('custom-ui:errors.component_too_large'),
    runtime_not_native: () => t('custom-ui:errors.runtime_not_native'),
    name_invalid: () => t('custom-ui:errors.name_invalid'),
    entry_invalid: () => t('custom-ui:errors.entry_invalid'),
    sdk_unsupported: () => t('custom-ui:errors.sdk_unsupported'),
    host_too_old: () => t('custom-ui:errors.host_too_old'),
    placements_invalid: () => t('custom-ui:errors.placements_invalid'),
    capability_unknown: () => t('custom-ui:errors.capability_unknown'),
    network_host_invalid: () => t('custom-ui:errors.network_host_invalid'),
    props_schema_invalid: () => t('custom-ui:errors.props_schema_invalid'),
    props_type_unsupported: () => t('custom-ui:errors.props_type_unsupported'),
    // 属性值（Runtime 保存实例时的逐字段校验）
    props_type_invalid: () => t('custom-ui:errors.props_type_invalid'),
    props_too_long: () => t('custom-ui:errors.props_too_long', { max }),
    props_enum_invalid: () => t('custom-ui:errors.props_enum_invalid'),
    props_url_invalid: () => t('custom-ui:errors.props_url_invalid'),
    props_color_invalid: () => t('custom-ui:errors.props_color_invalid'),
    props_out_of_range: () => t('custom-ui:errors.props_out_of_range'),
    props_too_many: () => t('custom-ui:errors.props_too_many', { max }),
    props_unknown: () => t('custom-ui:errors.props_unknown'),
    props_required: () => t('custom-ui:errors.props_required'),
    props_too_large: () => t('custom-ui:errors.props_too_large'),
    // 工作室操作（模板创建、启用、放置、删除）
    component_not_found: () => t('custom-ui:errors.component_not_found'),
    component_name_invalid: () => t('custom-ui:errors.component_name_invalid'),
    component_id_invalid: () => t('custom-ui:errors.component_id_invalid'),
    component_already_installed: () => t('custom-ui:errors.component_already_installed'),
    component_disabled: () => t('custom-ui:errors.component_disabled'),
    component_invalid: () => t('custom-ui:errors.component_invalid'),
    component_surface_unsupported: () => t('custom-ui:errors.component_surface_unsupported'),
    component_scope_invalid: () => t('custom-ui:errors.component_scope_invalid'),
    component_title_invalid: () => t('custom-ui:errors.component_title_invalid'),
    component_props_invalid: () => t('custom-ui:errors.component_props_invalid'),
    component_instance_exists: () => t('custom-ui:errors.component_instance_exists'),
    component_instance_not_found: () => t('custom-ui:errors.component_instance_not_found'),
    component_instances_full: () => t('custom-ui:errors.component_instances_full'),
  }
  return labels[code]?.() ?? code
}
