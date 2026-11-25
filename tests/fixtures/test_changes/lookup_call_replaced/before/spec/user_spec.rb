RSpec.describe User do
  it "finds by account" do
    found = User.where(account_id: id).first
    expect(found).to eq(account)
  end
end
