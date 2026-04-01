variable "aws_region" {
  description = "AWS region to deploy into"
  type        = string
  default     = "us-east-1"
}

variable "vpc_id" {
  description = "VPC ID to deploy into"
  type        = string
}

variable "subnet_ids" {
  description = "List of subnet IDs for the NLB and ECS tasks (use public subnets if tasks need outbound internet)"
  type        = list(string)
}

variable "image_tag" {
  description = "Docker image tag to deploy (e.g. latest, v1.2.3, git SHA)"
  type        = string
  default     = "latest"
}

variable "route53_zone_id" {
  description = "Route 53 hosted zone ID"
  type        = string
}

variable "dns_name" {
  description = "Fully qualified domain name for the server (e.g. tcpunch.example.com)"
  type        = string
}

variable "ecs_cluster_name" {
  description = "Name of an existing ECS Fargate cluster to deploy into"
  type        = string
  default     = "CylonFargateExperiments"
}

variable "desired_count" {
  description = "Number of ECS tasks to run (use 1 for single-node, >1 requires redis_url)"
  type        = number
  default     = 1
}

variable "task_cpu" {
  description = "CPU units for the ECS task (256 = 0.25 vCPU)"
  type        = number
  default     = 256
}

variable "task_memory" {
  description = "Memory in MiB for the ECS task"
  type        = number
  default     = 512
}

variable "redis_url" {
  description = "Redis connection URL for multi-node mode (leave empty for single-node in-memory mode)"
  type        = string
  default     = ""
  sensitive   = true
}

variable "peer_timeout" {
  description = "Seconds to wait for a peer to connect"
  type        = number
  default     = 300
}

variable "max_connections" {
  description = "Maximum concurrent connections per task"
  type        = number
  default     = 10000
}

variable "log_level" {
  description = "Log level for the server (trace, debug, info, warn, error)"
  type        = string
  default     = "info"
}

variable "name" {
  description = "Name prefix for all resources"
  type        = string
  default     = "tcpunch"
}