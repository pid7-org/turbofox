variable "aws_region" {
  description = "AWS deployment region"
  type        = string
  default     = "us-east-1"
}

variable "instance_type" {
  description = "Compute-optimized x86_64 instance with dedicated NVMe"
  type        = string
  default     = "c5d.large"
}

variable "ops" {
  description = "Number of benchmark operations per suite"
  type        = string
  default     = "0x186A0"
}

variable "project_tag" {
  description = "Resource tracking tag"
  type        = string
  default     = "turbofox-bencher"
}
