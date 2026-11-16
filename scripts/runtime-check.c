#include <locale.h>
#include <stdio.h>
#include <stdlib.h>
#include <xkbcommon/xkbcommon.h>
#include <xkbcommon/xkbcommon-compose.h>
int main(void) {
    if (!setlocale(LC_ALL,"")) {fputs("Unsupported locale\n",stderr);return 10;}
    struct xkb_context *ctx=xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    if (!ctx) {fputs("XKB context unavailable\n",stderr);return 11;}
    struct xkb_rule_names names={.rules="evdev",.model="pc105",.layout="us,ru"};
    struct xkb_keymap *map=xkb_keymap_new_from_names(ctx,&names,XKB_KEYMAP_COMPILE_NO_FLAGS);
    if (!map) {xkb_context_unref(ctx);return 12;}
    struct xkb_compose_table *compose=xkb_compose_table_new_from_locale(ctx,setlocale(LC_CTYPE,NULL),XKB_COMPOSE_COMPILE_NO_FLAGS);
    if (!compose) {xkb_keymap_unref(map);xkb_context_unref(ctx);return 13;}
    xkb_compose_table_unref(compose);
    xkb_keymap_unref(map);xkb_context_unref(ctx);puts("XKB us,ru, Compose and locale: PASS");return 0;
}
