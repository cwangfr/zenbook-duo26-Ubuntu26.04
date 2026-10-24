// GNOME Settings 50 numbers monitors after reversing built-in order.
// cc_display_config_constructed prepends each is-builtin monitor, which
// reverses Mutter's list: eDP-1 then eDP-2 becomes UI 2 then UI 1.
// ponytail: eDP- prefix stands in for Mutter's is-builtin flag. A built-in
// whose connector is not eDP- is numbered with the external monitors.
export function gnomeUiNumbers(connectors: readonly string[]): Map<string, number> {
  const builtins: string[] = [];
  const external: string[] = [];
  for (const connector of connectors) {
    if (connector.startsWith("eDP")) builtins.push(connector);
    else external.push(connector);
  }
  builtins.reverse();
  return new Map(builtins.concat(external).map((connector, index) => [connector, index + 1]));
}
