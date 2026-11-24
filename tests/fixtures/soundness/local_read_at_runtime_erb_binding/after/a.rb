def f(y)
  ERB.new('<%= x %>').result(binding)
end
