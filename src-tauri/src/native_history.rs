//! Native Win32 capture-history window.
#![cfg(windows)]

use std::{ffi::c_void, mem, path::PathBuf, ptr, sync::Once};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
            GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor, AC_SRC_ALPHA, AC_SRC_OVER,
            ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
            CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CENTER, DT_END_ELLIPSIS,
            DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, HBITMAP, HDC,
            HGDIOBJ, OUT_DEFAULT_PRECIS, TRANSPARENT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{
                TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DELETE, VK_ESCAPE, VK_LEFT,
                VK_RIGHT,
            },
            WindowsAndMessaging::*,
        },
    },
};

const CLASS_NAME: &[u16] = &[80,117,114,101,83,104,111,116,72,105,115,116,111,114,121,0];
const WM_MOUSELEAVE: u32 = 0x02A3;
const BASE_W: i32 = 900;
const BASE_H: i32 = 500;
const CARD_W: i32 = 270;
const CARD_H: i32 = 180;
const CARD_GAP: i32 = 24;
static REGISTER: Once = Once::new();

#[derive(Clone)]
pub struct HistoryEntry {
    pub id: String,
    pub path: PathBuf,
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub created_at: i64,
}

pub struct HistoryCallbacks {
    pub on_restore: Box<dyn Fn(HistoryEntry) + Send>,
    pub on_delete: Box<dyn Fn(HistoryEntry) + Send>,
}

struct State {
    items: Vec<HistoryEntry>,
    selected: usize,
    hovered: Option<usize>,
    width: i32,
    height: i32,
    scale: f32,
    offset_x: f32,
    surface: Vec<u8>,
    mem_dc: HDC,
    default_bitmap: HBITMAP,
    dib: HBITMAP,
    dib_bits: *mut u8,
    callbacks: HistoryCallbacks,
}

fn rounded_rect_sdf(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = px.abs() - hw + r;
    let qy = py.abs() - hh + r;
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r
}

fn blend(dst: &mut [u8], r: f32, g: f32, b: f32, a: f32) {
    let da = dst[3] as f32 / 255.0;
    let out_a = a + da * (1.0 - a);
    if out_a <= 0.0 { return; }
    let db = dst[0] as f32 / 255.0;
    let dg = dst[1] as f32 / 255.0;
    let dr = dst[2] as f32 / 255.0;
    dst[0] = ((b * a + db * (1.0 - a)) * 255.0) as u8;
    dst[1] = ((g * a + dg * (1.0 - a)) * 255.0) as u8;
    dst[2] = ((r * a + dr * (1.0 - a)) * 255.0) as u8;
    dst[3] = (out_a * 255.0) as u8;
}

fn card_x(index: usize, selected: usize) -> i32 {
    BASE_W / 2 - CARD_W / 2 + (index as i32 - selected as i32) * (CARD_W + CARD_GAP)
}

fn move_selection(state: &mut State, direction: i32) {
    if state.items.is_empty() { return; }
    if direction > 0 { state.selected = (state.selected + 1).min(state.items.len() - 1); }
    else if direction < 0 { state.selected = state.selected.saturating_sub(1); }
}

fn to_virtual(state: &State, x: i32, y: i32) -> (i32, i32) {
    (((x as f32 - state.offset_x) / state.scale) as i32, (y as f32 / state.scale) as i32)
}

fn hit_card(x: i32, y: i32, state: &State) -> Option<usize> {
    let (x, y) = to_virtual(state, x, y);
    if !(168..=348).contains(&y) { return None; }
    (0..state.items.len()).find(|&i| { let left = card_x(i, state.selected); x >= left && x <= left + CARD_W })
}

unsafe fn ensure_surface(state: &mut State) -> bool {
    if !state.surface.is_empty() { return true; }
    state.mem_dc = CreateCompatibleDC(None);
    if state.mem_dc.is_invalid() { return false; }
    let mut info = BITMAPINFO::default();
    info.bmiHeader = BITMAPINFOHEADER { biSize: mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: state.width, biHeight: -state.height, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, biSizeImage: (state.width * state.height * 4) as u32, ..Default::default() };
    let mut bits: *mut c_void = ptr::null_mut();
    let Ok(dib) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else { return false; };
    state.default_bitmap = HBITMAP(SelectObject(state.mem_dc, HGDIOBJ(dib.0)).0);
    state.dib = dib;
    state.dib_bits = bits as *mut u8;
    state.surface.resize((state.width * state.height * 4) as usize, 0);
    true
}

fn sample_cover(item: &HistoryEntry, x: i32, y: i32) -> (f32, f32, f32) {
    let iw = item.width.max(1) as f32;
    let ih = item.height.max(1) as f32;
    let scale = (CARD_W as f32 / iw).max(CARD_H as f32 / ih);
    let sx = ((x as f32 - (CARD_W as f32 - iw * scale) * 00.5) / scale).clamp(0.0, iw - 1.0) as u32;
    let sy = ((y as f32 - (CARD_H as f32 - ih * scale) * 00.5) / scale).clamp(0.0, ih - 1.0) as u32;
    let p = ((sy * item.width + sx) * 4) as usize;
    (item.rgba[p] as f32 / 255.0, item.rgba[p+1] as f32 / 255.0, item.rgba[p+2] as f32 / 255.0)
}

unsafe fn render(hwnd: HWND, state: &mut State) {
    if !ensure_surface(state) { return; }
    // Start fully transparent so only the History controls and screenshot
    // cards are visible; the desktop remains untouched everywhere else.
    state.surface.fill(0);
    for (left, right, active) in [(183,257,true),(277,453,false)] {
        let x0=(state.offset_x+left as f32*state.scale).floor() as i32; let x1=(state.offset_x+right as f32*state.scale).ceil() as i32;
        let y0=(82.0*state.scale).floor() as i32; let y1=(120.0*state.scale).ceil() as i32;
        for y in y0..y1 { for x in x0..x1 { let vx=(x as f32-state.offset_x)/state.scale; let vy=y as f32/state.scale; let i=((y*state.width+x)*4) as usize; let sdf=rounded_rect_sdf(vx-(left+right) as f32*0.5,vy-101.0,(right-left) as f32*0.5,19.0,19.0); if sdf<00.5 { let a=(00.5-sdf).clamp(0.0,1.0); if active { blend(&mut state.surface[i..i+4],0.02,0.48,1.0,a) } else { blend(&mut state.surface[i..i+4],0.30,0.30,0.32,a*0.75) } } }}
    }
    for (index,item) in state.items.iter().enumerate() {
        let left=card_x(index,state.selected); let selected=index==state.selected; let hovered=state.hovered==Some(index);
        let x0=(state.offset_x+left as f32*state.scale).max(0.0).floor() as i32; let x1=(state.offset_x+(left+CARD_W) as f32*state.scale).min(state.width as f32).ceil() as i32;
        let y0=(168.0*state.scale).floor() as i32; let y1=(348.0*state.scale).min(state.height as f32).ceil() as i32;
        for y in y0..y1 { for x in x0..x1 { let cx=(x as f32-state.offset_x)/state.scale-left as f32; let cy=y as f32/state.scale-168.0; let i=((y*state.width+x)*4) as usize; let sdf=rounded_rect_sdf(cx-CARD_W as f32*0.5,cy-CARD_H as f32*0.5,CARD_W as f32*0.5,CARD_H as f32*0.5,22.0); let cover=(00.5-sdf).clamp(0.0,1.0); if cover>0.0 { let(mut r,mut g,mut b)=sample_cover(item,cx as i32,cy as i32); if hovered {r*=0.78;g*=0.78;b*=0.78;} blend(&mut state.surface[i..i+4],r,g,b,cover); } if selected { let border=(40.5-sdf.abs()).clamp(0.0,1.0); if border>0.0 { blend(&mut state.surface[i..i+4],0.02,00.50,1.0,border); } } }}
    }
    if !state.items.is_empty() { let x0=(state.offset_x+376.0*state.scale) as i32; let x1=(state.offset_x+524.0*state.scale) as i32; let y0=(386.0*state.scale) as i32; let y1=(425.0*state.scale) as i32; for y in y0..y1 { for x in x0..x1 { let vx=(x as f32-state.offset_x)/state.scale; let vy=y as f32/state.scale; let i=((y*state.width+x)*4) as usize; let sdf=rounded_rect_sdf(vx-450.0,vy-4050.5,74.0,190.5,12.0); if sdf<0.5 {blend(&mut state.surface[i..i+4],0.02,0.48,1.0,(0.5-sdf).clamp(0.0,1.0));} }} }
    // Render at a larger virtual canvas and let the layered window use the
    // monitor-sized surface; this keeps rounded cards and text edges smooth.
    ptr::copy_nonoverlapping(state.surface.as_ptr(),state.dib_bits,state.surface.len());
    draw_text(state);
    // GDI does not maintain the alpha byte of a 32-bit DIB. Text therefore
    // has RGB values but zero alpha, which UpdateLayeredWindow composites as
    // transparent (or as black on some DWM/compositor paths). Make GDI's
    // antialiased text pixels opaque while retaining the compositor alpha
    // already written by our software renderer everywhere else.
    for pixel in std::slice::from_raw_parts_mut(state.dib_bits, state.surface.len()).chunks_exact_mut(4) {
        if pixel[3] == 0 && (pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0) {
            pixel[3] = pixel[0].max(pixel[1]).max(pixel[2]);
        }
    }
    let mut rect=RECT::default(); let _=GetWindowRect(hwnd,&mut rect); let dst=POINT{x:rect.left,y:rect.top}; let src=POINT{x:0,y:0}; let size=SIZE{cx:state.width,cy:state.height}; let bf=BLENDFUNCTION{BlendOp:AC_SRC_OVER as u8,BlendFlags:0,SourceConstantAlpha:255,AlphaFormat:AC_SRC_ALPHA as u8}; let screen=GetDC(None); let _=UpdateLayeredWindow(hwnd,Some(screen),Some(&dst),Some(&size),Some(state.mem_dc),Some(&src),COLORREF(0),Some(&bf),ULW_ALPHA); ReleaseDC(None,screen);
}

fn scaled_rect(state:&State,left:i32,top:i32,right:i32,bottom:i32)->RECT { RECT{left:(state.offset_x+left as f32*state.scale) as i32,top:(top as f32*state.scale) as i32,right:(state.offset_x+right as f32*state.scale) as i32,bottom:(bottom as f32*state.scale) as i32} }
unsafe fn text(state:&State,value:&str,mut rect:RECT,size:i32,weight:u32,flags:windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT){let font=CreateFontW(-(size as f32*state.scale).round().max(9.0) as i32,0,0,0,weight as i32,0,0,0,DEFAULT_CHARSET,OUT_DEFAULT_PRECIS,CLIP_DEFAULT_PRECIS,ANTIALIASED_QUALITY,FF_DONTCARE.0 as u32,windows::core::w!("Segoe UI"));let old=SelectObject(state.mem_dc,HGDIOBJ(font.0));let wide:Vec<u16>=value.encode_utf16().collect();DrawTextW(state.mem_dc,&mut wide.clone(),&mut rect,flags);SelectObject(state.mem_dc,old);let _=DeleteObject(HGDIOBJ(font.0));}
unsafe fn draw_text(state:&State){SetBkMode(state.mem_dc,TRANSPARENT);SetTextColor(state.mem_dc,COLORREF(0x00ffffff));text(state,"All",scaled_rect(state,183,82,257,120),20,FW_SEMIBOLD.0,DT_CENTER|DT_VCENTER|DT_SINGLELINE);text(state,"Screenshots",scaled_rect(state,277,82,453,120),20,FW_SEMIBOLD.0,DT_CENTER|DT_VCENTER|DT_SINGLELINE);if state.items.is_empty(){text(state,"No recent captures",scaled_rect(state,0,210,BASE_W,250),22,FW_SEMIBOLD.0,DT_CENTER|DT_VCENTER|DT_SINGLELINE)}else{text(state,"↩  Restore",scaled_rect(state,376,386,524,425),18,FW_SEMIBOLD.0,DT_CENTER|DT_VCENTER|DT_SINGLELINE);let item=&state.items[state.selected];let name=item.path.file_name().and_then(|v|v.to_str()).unwrap_or("Screenshot");text(state,name,scaled_rect(state,260,438,640,454),12,FW_NORMAL.0,DT_CENTER|DT_VCENTER|DT_SINGLELINE|DT_END_ELLIPSIS)}}
unsafe fn restore(hwnd:HWND,state:&mut State){if let Some(item)=state.items.get(state.selected).cloned(){(state.callbacks.on_restore)(item);let _=DestroyWindow(hwnd);}}
unsafe fn delete_selected(hwnd:HWND,state:&mut State){if state.items.is_empty(){return}let item=state.items.remove(state.selected);(state.callbacks.on_delete)(item);state.selected=state.selected.min(state.items.len().saturating_sub(1));render(hwnd,state);}
unsafe extern "system" fn wndproc(hwnd:HWND,msg:u32,w:WPARAM,l:LPARAM)->LRESULT{if msg==WM_NCCREATE{let c=&*(l.0 as *const CREATESTRUCTW);SetWindowLongPtrW(hwnd,GWLP_USERDATA,c.lpCreateParams as isize);}let raw=GetWindowLongPtrW(hwnd,GWLP_USERDATA) as *mut State;match msg{WM_SIZE if !raw.is_null()=>render(hwnd,&mut*raw),WM_MOUSEMOVE if !raw.is_null()=>{let x=(l.0 as i16)as i32;let y=((l.0>>16)as i16)as i32;let s=&mut*raw;let next=hit_card(x,y,s);if next!=s.hovered{s.hovered=next;render(hwnd,s)}let mut t=TRACKMOUSEEVENT{cbSize:mem::size_of::<TRACKMOUSEEVENT>()as u32,dwFlags:TME_LEAVE,hwndTrack:hwnd,dwHoverTime:0};let _=TrackMouseEvent(&mut t);},WM_MOUSELEAVE if !raw.is_null()=>{(*raw).hovered=None;render(hwnd,&mut*raw)},WM_LBUTTONUP if !raw.is_null()=>{let x=(l.0 as i16)as i32;let y=((l.0>>16)as i16)as i32;let s=&mut*raw;if let Some(i)=hit_card(x,y,s){s.selected=i;render(hwnd,s)}else{let(vx,vy)=to_virtual(s,x,y);if(376..=524).contains(&vx)&&(386..=425).contains(&vy){restore(hwnd,s)}}},WM_LBUTTONDBLCLK if !raw.is_null()=>restore(hwnd,&mut*raw),WM_KEYDOWN if !raw.is_null()=>{let s=&mut*raw;if w.0==VK_ESCAPE.0 as usize{let _=DestroyWindow(hwnd);}else if w.0==VK_DELETE.0 as usize{delete_selected(hwnd,s)}else if w.0==VK_LEFT.0 as usize{move_selection(s,-1);render(hwnd,s)}else if w.0==VK_RIGHT.0 as usize{move_selection(s,1);render(hwnd,s)}},WM_MOUSEWHEEL if !raw.is_null()=>{let s=&mut*raw;let delta=((w.0>>16)&0xffff) as i16 as i32;move_selection(s,if delta<0{1}else{-1});render(hwnd,s)},WM_MOUSEHWHEEL if !raw.is_null()=>{let s=&mut*raw;let delta=((w.0>>16)&0xffff) as i16 as i32;move_selection(s,if delta<0{1}else{-1});render(hwnd,s)},WM_POINTERWHEEL if !raw.is_null()=>{let s=&mut*raw;let delta=((w.0>>16)&0xffff) as i16 as i32;move_selection(s,if delta<0{1}else{-1});render(hwnd,s)},WM_CLOSE=>{let _=DestroyWindow(hwnd);return LRESULT(0)},WM_NCDESTROY if !raw.is_null()=>{SetWindowLongPtrW(hwnd,GWLP_USERDATA,0);if !(*raw).dib.is_invalid(){SelectObject((*raw).mem_dc,HGDIOBJ((*raw).default_bitmap.0));let _=DeleteObject(HGDIOBJ((*raw).dib.0));}if !(*raw).mem_dc.is_invalid(){let _=DeleteDC((*raw).mem_dc);}drop(Box::from_raw(raw));PostQuitMessage(0);return LRESULT(0)},_=>{}}DefWindowProcW(hwnd,msg,w,l)}
fn register_class()->Result<(),String>{let mut error=None;REGISTER.call_once(||unsafe{let module=GetModuleHandleW(None).unwrap_or_default();let class=WNDCLASSW{lpfnWndProc:Some(wndproc),hInstance:HINSTANCE(module.0),hCursor:LoadCursorW(None,IDC_ARROW).unwrap_or_default(),lpszClassName:PCWSTR(CLASS_NAME.as_ptr()),style:CS_HREDRAW|CS_VREDRAW|CS_DBLCLKS,..Default::default()};if RegisterClassW(&class)==0{error=Some("Unable to register native history window".into())}});error.map_or(Ok(()),Err)}

pub fn create(items:Vec<HistoryEntry>,monitor_x:i32,monitor_y:i32,monitor_w:i32,monitor_h:i32,callbacks:HistoryCallbacks)->Result<(),String>{
    register_class()?;
    std::thread::spawn(move||unsafe{
        // The native window is only the bottom history shelf. Keeping the
        // window to this rect means the desktop above remains untouched and
        // avoids opaque black bands around a fullscreen layered surface.
        let width=monitor_w.max(1); let height=((monitor_h as f32*0.30).round() as i32).max(220); let scale=height as f32/BASE_H as f32; let offset_x=(width as f32-BASE_W as f32*scale)*0.5;
        let state=Box::new(State{items,selected:0,hovered:None,width,height,scale,offset_x,surface:Vec::new(),mem_dc:HDC(ptr::null_mut()),default_bitmap:HBITMAP(ptr::null_mut()),dib:HBITMAP(ptr::null_mut()),dib_bits:ptr::null_mut(),callbacks});let raw=Box::into_raw(state);let x=monitor_x;let y=monitor_y+monitor_h-height;
        let hwnd=CreateWindowExW(WS_EX_TOPMOST|WS_EX_TOOLWINDOW|WS_EX_LAYERED,PCWSTR(CLASS_NAME.as_ptr()),windows::core::w!("PureShot History"),WS_POPUP,x,y,width,height,None,None,None,Some(raw as *const c_void));let Ok(hwnd)=hwnd else{drop(Box::from_raw(raw));return};render(hwnd,&mut*raw);let _=SetWindowPos(hwnd,Some(HWND_TOPMOST),x,y,width,height,SWP_SHOWWINDOW);let _=SetForegroundWindow(hwnd);let mut msg=MSG::default();while GetMessageW(&mut msg,None,0,0).as_bool(){let _=TranslateMessage(&msg);DispatchMessageW(&msg);}
    });Ok(())
}
