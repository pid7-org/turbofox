output "instance_id" {
  description = "Target EC2 instance identifier"
  value       = aws_instance.bencher.id
}

output "instance_public_ip" {
  description = "Public IP address of benchmark host"
  value       = aws_instance.bencher.public_ip
}

output "ssh_private_key_pem" {
  description = "Ephemeral private key for SSH authentication"
  value       = tls_private_key.ssh.private_key_openssh
  sensitive   = true
}

output "ssh_command" {
  description = "Convenience SSH command string"
  value       = format("ssh -i id_ed25519 ubuntu@%s", aws_instance.bencher.public_ip)
}
