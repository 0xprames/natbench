//! STUN Binding (RFC 5389) for IPv4, without authentication.
//!
//! The client sends a Binding Request and reads XOR-MAPPED-ADDRESS on that same
//! socket. The lab server answers from the address the request was sent to.

const MAGIC: u32 = 0x2112_A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;

pub fn binding_request(transaction: &[u8; 12]) -> [u8; 20] {
    let mut message = [0u8; 20];
    message[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    message[4..8].copy_from_slice(&MAGIC.to_be_bytes());
    message[8..20].copy_from_slice(transaction);
    message
}

pub fn binding_success(transaction: &[u8; 12], ip: [u8; 4], port: u16) -> [u8; 32] {
    let mut message = [0u8; 32];
    message[0..2].copy_from_slice(&BINDING_SUCCESS.to_be_bytes());
    message[2..4].copy_from_slice(&12u16.to_be_bytes());
    message[4..8].copy_from_slice(&MAGIC.to_be_bytes());
    message[8..20].copy_from_slice(transaction);
    message[20..22].copy_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
    message[22..24].copy_from_slice(&8u16.to_be_bytes());
    message[25] = 0x01;
    let xport = port ^ (MAGIC >> 16) as u16;
    message[26..28].copy_from_slice(&xport.to_be_bytes());
    let cookie = MAGIC.to_be_bytes();
    for (index, byte) in ip.into_iter().enumerate() {
        message[28 + index] = byte ^ cookie[index];
    }
    message
}

pub fn transaction_id(packet: &[u8]) -> Option<[u8; 12]> {
    if packet.len() < 20 || u16::from_be_bytes([packet[0], packet[1]]) != BINDING_REQUEST {
        return None;
    }
    if u32::from_be_bytes(packet[4..8].try_into().ok()?) != MAGIC {
        return None;
    }
    let mut id = [0u8; 12];
    id.copy_from_slice(&packet[8..20]);
    Some(id)
}

pub fn mapped_address(packet: &[u8], transaction: &[u8; 12]) -> Option<([u8; 4], u16)> {
    if packet.len() < 32 || u16::from_be_bytes([packet[0], packet[1]]) != BINDING_SUCCESS {
        return None;
    }
    if u32::from_be_bytes(packet[4..8].try_into().ok()?) != MAGIC || &packet[8..20] != transaction {
        return None;
    }
    let declared = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    let end = (20 + declared).min(packet.len());
    let mut offset = 20;
    while offset + 4 <= end {
        let attribute = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let length = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;
        let value = offset + 4;
        if value + length > end {
            return None;
        }
        if attribute == XOR_MAPPED_ADDRESS && length >= 8 && packet[value + 1] == 0x01 {
            let port =
                u16::from_be_bytes([packet[value + 2], packet[value + 3]]) ^ (MAGIC >> 16) as u16;
            let cookie = MAGIC.to_be_bytes();
            let ip = [
                packet[value + 4] ^ cookie[0],
                packet[value + 5] ^ cookie[1],
                packet[value + 6] ^ cookie[2],
                packet[value + 7] ^ cookie[3],
            ];
            return Some((ip, port));
        }
        offset = value + ((length + 3) & !3);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_mapped_address_is_not_written_in_the_clear() {
        let transaction = [9u8; 12];
        let request = binding_request(&transaction);
        assert_eq!(transaction_id(&request), Some(transaction));
        let response = binding_success(&transaction, [198, 18, 0, 10], 10000);
        assert_ne!(&response[28..32], &[198, 18, 0, 10]);
        assert_eq!(
            mapped_address(&response, &transaction),
            Some(([198, 18, 0, 10], 10000))
        );
        let mut other = transaction;
        other[0] = 0;
        assert_eq!(mapped_address(&response, &other), None);
    }
}
