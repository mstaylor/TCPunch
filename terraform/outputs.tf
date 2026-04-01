output "ecr_repository_url" {
  description = "ECR repository URL — use this to push images"
  value       = aws_ecr_repository.rendezvous.repository_url
}

output "nlb_dns_name" {
  description = "NLB DNS name"
  value       = aws_lb.rendezvous.dns_name
}

output "server_dns_name" {
  description = "Route 53 DNS name for the server"
  value       = var.dns_name
}

output "ecs_cluster_name" {
  description = "ECS cluster name"
  value       = data.aws_ecs_cluster.rendezvous.cluster_name
}

output "ecs_service_name" {
  description = "ECS service name"
  value       = aws_ecs_service.rendezvous.name
}

output "cloudwatch_log_group" {
  description = "CloudWatch log group for ECS task logs"
  value       = aws_cloudwatch_log_group.rendezvous.name
}