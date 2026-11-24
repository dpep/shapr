def build(name)
  Object.const_get(name).new
end
build(ARGV[0])
