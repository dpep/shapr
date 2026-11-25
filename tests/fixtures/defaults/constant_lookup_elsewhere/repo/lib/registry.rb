class Registry
  def lookup(name)
    Object.const_get(name)
  end
end
