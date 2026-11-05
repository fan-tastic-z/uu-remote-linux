#include <stdint.h>
/* Map upright output pixels to untransformed capture/device coordinates (Smithay Transform). */
static inline void uu_source_pixel(int transform,uint32_t w,uint32_t h,uint32_t x,uint32_t y,uint32_t *sx,uint32_t *sy) {
    *sx=x;*sy=y;
    switch(transform) {
        case 1:*sx=y;*sy=h-1-x;break;
        case 2:*sx=w-1-x;*sy=h-1-y;break;
        case 3:*sx=w-1-y;*sy=x;break;
    }
}
/* Rotated capture and pointer mapping had hardware acceptance only on niri; Hyprland 0.56
 * verified for 90deg on a headless output (upright SHM copy + VAAPI encode, 2026-11):
 * screencopy delivers the untransformed buffer and the SHM copy path (ungated) rotates.
 * The pointer branch stays compositor-specific: niri maps to device coordinates, Hyprland's
 * virtual pointer takes logical coordinates with logical extents. Flipped variants (4-7)
 * stay unsupported. */
static inline int uu_output_transform_supported(int transform,int niri) {
    (void)niri;
    return transform==0 || (transform>=1 && transform<=3);
}
