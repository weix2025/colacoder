// Certificate generation utility
// This binary is used to generate certificates for testing purposes

use rcgen::{CertificateParams, DnType, KeyPair};
use std::fs;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    println!("Generating test certificates...");
    
    let mut params = CertificateParams::new(vec!["localhost".to_string()])?;
    params.distinguished_name.push(DnType::CommonName, "localhost");
    params.distinguished_name.push(DnType::OrganizationName, "TQUIC Test");
    
    // Generate key pair
    let key_pair = KeyPair::generate()?;
    
    // Generate self-signed certificate
    let cert = params.self_signed(&key_pair)?;
    
    // Write certificate file
    fs::write("temp_cert.pem", cert.pem())?;
    
    // Write private key file
    fs::write("temp_key.pem", key_pair.serialize_pem())?;
    
    println!("Certificates generated successfully:");
    println!("  - temp_cert.pem");
    println!("  - temp_key.pem");
    
    Ok(())
}