//! This module is for NAT Behavior Discovery based on RFC5780.
//! To use this module, the STUN server side must support the OTHER-ADDRESS and CHANGE-REQUEST attributes.
use std::collections::HashMap;

use async_std::net::{SocketAddr, ToSocketAddrs};
use local_ip_address::list_afinet_netifas;

use super::client::*;
use super::error::*;
use super::message::*;

/// Defines a NAT type based on mapping behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NATMappingType {
    NoNAT,
    EndpointIndependent,
    AddressDependent,
    AddressAndPortDependent,
    Unknown,
}

/// Defines a NAT type based on filtering behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NATFilteringType {
    EndpointIndependent,
    AddressDependent,
    AddressAndPortDependent,
    Unknown,
}

/// Results of behavior discovery based on NAT mapping behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NATMappingTypeResult {
    pub test1_xor_mapped_addr: Option<SocketAddr>,
    pub test2_xor_mapped_addr: Option<SocketAddr>,
    pub test3_xor_mapped_addr: Option<SocketAddr>,
    pub mapping_type: NATMappingType,
}

/// Results of behavior discovery based on NAT filtering behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NATFilteringTypeResult {
    pub xor_mapped_addr: Option<SocketAddr>,
    pub filtering_type: NATFilteringType,
}

/// Check NAT mapping behavior.
pub async fn check_nat_mapping_behavior<A: ToSocketAddrs>(
    client: &mut Client,
    stun_addr: A,
) -> Result<NATMappingTypeResult, STUNClientError> {
    let mut result = NATMappingTypeResult {
        test1_xor_mapped_addr: None,
        test2_xor_mapped_addr: None,
        test3_xor_mapped_addr: None,
        mapping_type: NATMappingType::Unknown,
    };

    // get NIC IPs
    let local_ips = list_afinet_netifas().unwrap();

    // Test1
    // Send a Binding request and check the Endpoint mapped to NAT.
    // Compare with the IP of the NIC and check if it is behind the NAT.
    let t1_res = client.binding_request(&stun_addr, None).await?;
    let other_addr = Attribute::get_other_address(&t1_res).ok_or(
        STUNClientError::NotSupportedError(String::from("OTHER-ADDRESS")),
    )?;
    result.test1_xor_mapped_addr = Some(Attribute::get_xor_mapped_address(&t1_res).ok_or(
        STUNClientError::NotSupportedError(String::from("XOR-MAPPED-ADDRESS")),
    )?);
    let addr = result.test1_xor_mapped_addr.unwrap().ip();
    for (_, local_ip) in local_ips {
        if local_ip == addr {
            result.mapping_type = NATMappingType::NoNAT;
            return Ok(result);
        }
    }

    // Test2
    // Send Binding Request to IP:Port of OTHER-ADDRESS.
    // Compare Test1 and Test2 XOR-MAPPED-ADDRESS to check if it is EIM-NAT.
    let t2_res = client.binding_request(&other_addr, None).await?;
    result.test2_xor_mapped_addr = Some(Attribute::get_xor_mapped_address(&t2_res).ok_or(
        STUNClientError::NotSupportedError(String::from("XOR-MAPPED-ADDRESS")),
    )?);
    if result.test1_xor_mapped_addr == result.test2_xor_mapped_addr {
        result.mapping_type = NATMappingType::EndpointIndependent;
        return Ok(result);
    }

    // Test3
    // Send a Binding Request to the IP used in Test1 and the Port used in Test2.
    // (That is, use the primary IP and secondary Port.)
    // Compare Test2 and Test3 XOR-MAPPED-ADDRESS to check if it is ADM-NAT or APDM-NAT.
    // stun_addr is a known value, so it's okay to unwrap it.
    let mut t3_addr = stun_addr.to_socket_addrs().await.unwrap().next().unwrap();
    t3_addr.set_port(other_addr.port());
    let t3_res = client.binding_request(&t3_addr, None).await?;
    result.test3_xor_mapped_addr = Some(Attribute::get_xor_mapped_address(&t3_res).ok_or(
        STUNClientError::NotSupportedError(String::from("XOR-MAPPED-ADDRESS")),
    )?);
    if result.test2_xor_mapped_addr == result.test3_xor_mapped_addr {
        result.mapping_type = NATMappingType::AddressDependent;
        return Ok(result);
    }

    result.mapping_type = NATMappingType::AddressAndPortDependent;
    Ok(result)
}

/// Check NAT filtering behavior.
pub async fn check_nat_filtering_behavior<A: ToSocketAddrs>(
    client: &mut Client,
    stun_addr: A,
) -> Result<NATFilteringTypeResult, STUNClientError> {
    // Test1
    // Send a Binding request and check the Endpoint mapped to NAT.
    let t1_res = client.binding_request(&stun_addr, None).await?;
    let xor_mapped_addr = Some(Attribute::get_xor_mapped_address(&t1_res).ok_or(
        STUNClientError::NotSupportedError(String::from("XOR-MAPPED-ADDRESS")),
    )?);

    // Test2
    // Send Binding Request with the "change IP" and "change port" flags of CHANGE-REQUEST turned on.
    // As a result, the response is sent from IP:Port which is different from the sent IP:Port.
    // If the response can be received, it is EIF-NAT.
    let mut attrs = HashMap::new();
    let change_request = Attribute::generate_change_request_value(true, true);
    attrs.insert(Attribute::ChangeRequest, change_request);
    let t2_res = client.binding_request(&stun_addr, Some(attrs)).await;
    match t2_res {
        Ok(_) => {
            return Ok(NATFilteringTypeResult {
                xor_mapped_addr: xor_mapped_addr,
                filtering_type: NATFilteringType::EndpointIndependent,
            })
        }
        Err(e) => {
            match e {
                STUNClientError::TimeoutError() => { /* Run Test3 below */ }
                _ => return Err(e),
            }
        }
    }

    // Test3
    // Send a binding request with only the "change port" flag in CHANGE-REQUEST turned on.
    // As a result, the response is sent from Port which is different from the sent Port.(Same IP address)
    // If the response can be received, it is ADF-NAT, and if it cannot be received, it is APDF-NAT.
    let mut attrs = HashMap::new();
    let change_request = Attribute::generate_change_request_value(false, true);
    attrs.insert(Attribute::ChangeRequest, change_request);
    let t3_res = client.binding_request(&stun_addr, Some(attrs)).await;
    match t3_res {
        Ok(_) => {
            return Ok(NATFilteringTypeResult {
                xor_mapped_addr: xor_mapped_addr,
                filtering_type: NATFilteringType::AddressDependent,
            })
        }
        Err(e) => match e {
            STUNClientError::TimeoutError() => {
                return Ok(NATFilteringTypeResult {
                    xor_mapped_addr: xor_mapped_addr,
                    filtering_type: NATFilteringType::AddressAndPortDependent,
                })
            }
            _ => return Err(e),
        },
    }
}
