def f(name)
  <<~EOS
    a #{name}
  #{name} b
  EOS
end
