use super::*;

const TOKEN: &str = "sk-ant-oat01-Ab3_x-9ZqLmN0pQrStUvWxYz0123456789";

#[test]
fn the_token_is_read_from_between_cursor_moves() {
    let screen = format!(
        "\x1b[2GYour\x1b[7GOAuth\x1b[13Gtoken\x1b[19G(valid\x1b[26Gfor\x1b[30G1\x1b[32Gyear):\r\r\n\r\r\n\x1b[2G{TOKEN}\x1b[120GStore\x1b[126Gthis\r\r\n"
    );
    assert_eq!(token_in(screen.as_bytes()).as_deref(), Some(TOKEN));
}

#[test]
fn a_link_or_a_bare_prefix_is_not_a_token() {
    let screen = "\x1b]8;id=1;https://claude.com/cai/oauth/authorize?code=true\x07sign in\x1b]8;;\x07 sk-ant-oat01-\r\n";
    assert_eq!(token_in(screen.as_bytes()), None);
    assert_eq!(token_in(b"Opening browser to sign in"), None);
}

#[test]
fn a_token_is_cut_at_the_first_character_outside_its_alphabet() {
    let screen = format!("{TOKEN}\"'; rm -rf ~\n");
    assert_eq!(token_in(screen.as_bytes()).as_deref(), Some(TOKEN));
}
