def build(name)
  name.safe_constantize&.new
end
build(ARGV[0])
