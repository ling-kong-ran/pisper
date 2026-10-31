// 组件错误码 → 文案（计划 §3.2：协议只给 code + params，由前端按 custom-ui:errors.<code> 本地化）。
// check-i18n 只认 t() 的字面量键，所以逐个写成静态表；表里没有的码原样显示，不伪装成别的错误。
// custom-ui-props-form.test.mjs 核对：shared 的 manifest 校验与工作室会碰到的 Runtime 错误码都在表里，
// 新增错误码忘了配文案会直接失败。
type Translate = (key: string, values?: Record<string, string | number>) => string
type Params = Record<string, string | number> | undefined

// 参数来自 Runtime（已校验的结构化错误），只取字符串 / 数字，其余按空串处理，避免把对象拼进文案。
function param(params: Params, name: string): string | number {
  const value = params?.[name]
  return typeof value === 'string' || typeof value === 'number' ? value : ''
}

// 写入上限以字节给出，显示成 KB 更好读。
function kilobytes(params: Params): string | number {
  const max = param(params, 'max')
  return typeof max === 'number' ? Math.round(max / 1024) : max
}

export function componentErrorText(code: string, t: Translate, params?: Params): string {
  const max = param(params, 'max')
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
    version_invalid: () => t('custom-ui:errors.version_invalid'),
    description_invalid: () => t('custom-ui:errors.description_invalid'),
    icon_invalid: () => t('custom-ui:errors.icon_invalid'),
    entry_invalid: () => t('custom-ui:errors.entry_invalid'),
    styles_invalid: () => t('custom-ui:errors.styles_invalid'),
    sdk_invalid: () => t('custom-ui:errors.sdk_invalid'),
    sdk_unsupported: () => t('custom-ui:errors.sdk_unsupported'),
    min_host_version_invalid: () => t('custom-ui:errors.min_host_version_invalid'),
    host_too_old: () =>
      params?.required
        ? t('custom-ui:errors.host_too_old_version', { required: param(params, 'required') })
        : t('custom-ui:errors.host_too_old'),
    placements_invalid: () => t('custom-ui:errors.placements_invalid'),
    placement_invalid: () => t('custom-ui:errors.placement_invalid'),
    placement_surface_invalid: () =>
      t('custom-ui:errors.placement_surface_invalid', { surface: param(params, 'surface') }),
    placement_duplicate: () =>
      t('custom-ui:errors.placement_duplicate', { surface: param(params, 'surface') }),
    placement_sizes_invalid: () => t('custom-ui:errors.placement_sizes_invalid'),
    placement_size_invalid: () =>
      t('custom-ui:errors.placement_size_invalid', {
        minW: param(params, 'minW'),
        maxW: param(params, 'maxW'),
        minH: param(params, 'minH'),
        maxH: param(params, 'maxH'),
      }),
    capability_invalid: () => t('custom-ui:errors.capability_invalid'),
    capability_unknown: () =>
      t('custom-ui:errors.capability_unknown', { capability: param(params, 'capability') }),
    capability_duplicate: () =>
      t('custom-ui:errors.capability_duplicate', { capability: param(params, 'capability') }),
    capability_reason_invalid: () => t('custom-ui:errors.capability_reason_invalid'),
    network_hosts_invalid: () => t('custom-ui:errors.network_hosts_invalid', { max }),
    network_host_invalid: () => t('custom-ui:errors.network_host_invalid'),
    storage_quota_invalid: () => t('custom-ui:errors.storage_quota_invalid', { max }),
    i18n_invalid: () => t('custom-ui:errors.i18n_invalid'),
    i18n_strings_invalid: () => t('custom-ui:errors.i18n_strings_invalid', { max }),
    i18n_string_invalid: () => t('custom-ui:errors.i18n_string_invalid'),
    // 属性声明（manifest.props）
    props_schema_invalid: () => t('custom-ui:errors.props_schema_invalid'),
    props_type_unsupported: () => t('custom-ui:errors.props_type_unsupported'),
    props_field_invalid: () => t('custom-ui:errors.props_field_invalid'),
    props_key_invalid: () => t('custom-ui:errors.props_key_invalid'),
    props_keyword_unsupported: () =>
      t('custom-ui:errors.props_keyword_unsupported', { keyword: param(params, 'keyword') }),
    props_format_invalid: () => t('custom-ui:errors.props_format_invalid'),
    props_max_length_invalid: () => t('custom-ui:errors.props_max_length_invalid', { max }),
    props_enum_list_invalid: () => t('custom-ui:errors.props_enum_list_invalid', { max }),
    props_number_invalid: () => t('custom-ui:errors.props_number_invalid'),
    props_unit_invalid: () => t('custom-ui:errors.props_unit_invalid'),
    props_items_invalid: () => t('custom-ui:errors.props_items_invalid'),
    props_max_items_invalid: () => t('custom-ui:errors.props_max_items_invalid', { max }),
    props_too_many_fields: () => t('custom-ui:errors.props_too_many_fields', { max }),
    props_required_invalid: () => t('custom-ui:errors.props_required_invalid'),
    props_default_invalid: () => t('custom-ui:errors.props_default_invalid'),
    props_i18n_invalid: () => t('custom-ui:errors.props_i18n_invalid'),
    props_title_invalid: () => t('custom-ui:errors.props_title_invalid'),
    props_description_invalid: () => t('custom-ui:errors.props_description_invalid'),
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
    // 写入（模板创建、代理 component_create 的结构化清单）
    component_files_invalid: () => t('custom-ui:errors.component_files_invalid'),
    component_manifest_invalid: () => t('custom-ui:errors.component_manifest_invalid'),
    component_file_path_invalid: () => t('custom-ui:errors.component_file_path_invalid'),
    component_file_duplicate: () => t('custom-ui:errors.component_file_duplicate'),
    component_file_content_invalid: () => t('custom-ui:errors.component_file_content_invalid'),
    component_file_too_large: () =>
      t('custom-ui:errors.component_file_too_large', { max: kilobytes(params) }),
    component_files_too_large: () =>
      t('custom-ui:errors.component_files_too_large', { max: kilobytes(params) }),
    component_files_too_many: () => t('custom-ui:errors.component_files_too_many', { max }),
    component_bare_import: () =>
      t('custom-ui:errors.component_bare_import', { specifier: param(params, 'specifier') }),
    component_html_unsupported: () => t('custom-ui:errors.component_html_unsupported'),
    path_not_writable: () => t('custom-ui:errors.path_not_writable'),
    component_template_invalid: () => t('custom-ui:errors.component_template_invalid'),
    component_id_reserved: () => t('custom-ui:errors.component_id_reserved'),
    component_id_taken_by_legacy: () => t('custom-ui:errors.component_id_taken_by_legacy'),
    component_legacy_unsupported: () => t('custom-ui:errors.component_legacy_unsupported'),
    component_import_unavailable: () => t('custom-ui:errors.component_import_unavailable'),
    component_request_invalid: () => t('custom-ui:errors.component_request_invalid'),
    component_request_too_large: () => t('custom-ui:errors.component_request_too_large'),
    component_asset_missing: () => t('custom-ui:errors.component_asset_missing'),
    component_capability_denied: () => t('custom-ui:errors.component_capability_denied'),
    // 工作室操作（模板创建、启用、放置、删除）
    component_not_found: () => t('custom-ui:errors.component_not_found'),
    component_name_invalid: () => t('custom-ui:errors.component_name_invalid'),
    component_id_invalid: () => t('custom-ui:errors.component_id_invalid'),
    component_already_installed: () => t('custom-ui:errors.component_already_installed'),
    component_disabled: () => t('custom-ui:errors.component_disabled'),
    component_invalid: () => t('custom-ui:errors.component_invalid'),
    component_surface_invalid: () => t('custom-ui:errors.component_surface_invalid'),
    component_surface_unsupported: () => t('custom-ui:errors.component_surface_unsupported'),
    component_size_invalid: () => t('custom-ui:errors.component_size_invalid'),
    component_scope_invalid: () => t('custom-ui:errors.component_scope_invalid'),
    component_title_invalid: () => t('custom-ui:errors.component_title_invalid'),
    component_props_invalid: () => t('custom-ui:errors.component_props_invalid'),
    component_instance_exists: () => t('custom-ui:errors.component_instance_exists'),
    component_instance_not_found: () => t('custom-ui:errors.component_instance_not_found'),
    component_instances_full: () => t('custom-ui:errors.component_instances_full'),
    component_instances_conflict: () => t('custom-ui:errors.component_instances_conflict'),
    component_instances_order_invalid: () =>
      t('custom-ui:errors.component_instances_order_invalid'),
  }
  return labels[code]?.() ?? code
}
